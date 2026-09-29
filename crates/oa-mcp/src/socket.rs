//! The local socket the GUI serves the tools on, and the bridge that joins
//! an agent's stdio to it. A Unix socket on Linux and macOS, a named pipe on
//! Windows; either way only the user's own processes can reach it.
use std::io;

/// Tools the GUI serves travel over this.
#[cfg(unix)]
pub type Stream = tokio::net::UnixStream;
#[cfg(windows)]
pub type Stream = tokio::net::windows::named_pipe::NamedPipeServer;

/// Where the GUI listens and `oa-mcp --attach` connects. `OA_SOCKET`
/// overrides the default, and `OA_SOCKET=off` gives `None`, which stops the
/// GUI from serving agents.
pub fn address() -> io::Result<Option<String>> {
    match std::env::var("OA_SOCKET") {
        Ok(v) if v.eq_ignore_ascii_case("off") => Ok(None),
        Ok(v) if !v.is_empty() => Ok(Some(v)),
        _ => default_address().map(Some),
    }
}

/// `$XDG_RUNTIME_DIR/open-analysis.sock`, which only the user can enter.
/// Without that variable, a socket in a directory of the user's own under
/// the temp directory, refused if anyone else owns it or can enter it.
#[cfg(unix)]
fn default_address() -> io::Result<String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        let path = std::path::Path::new(&dir).join("open-analysis.sock");
        return Ok(path.to_string_lossy().into_owned());
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let dir = std::env::temp_dir().join(format!("open-analysis-{uid}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is not a private directory of this user's; set OA_SOCKET to a path that is",
                dir.display()
            ),
        ));
    }
    Ok(dir.join("agent.sock").to_string_lossy().into_owned())
}

#[cfg(windows)]
fn default_address() -> io::Result<String> {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    Ok(format!(r"\\.\pipe\open-analysis-{user}"))
}

/// Accepts agent connections. Must be made and used inside a tokio runtime.
pub struct Listener {
    address: String,
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    /// The pipe instance the next client will connect to.
    #[cfg(windows)]
    next: tokio::net::windows::named_pipe::NamedPipeServer,
}

impl Listener {
    /// Binds `address`, replacing a socket file that nothing answers on.
    /// Fails with `AddrInUse` when another process is already serving.
    #[cfg(unix)]
    pub fn bind(address: &str) -> io::Result<Self> {
        use std::os::unix::fs::PermissionsExt;
        let path = std::path::Path::new(address);
        if path.exists() {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(in_use(address));
            }
            std::fs::remove_file(path)?;
        }
        // Socket paths are limited to about 100 bytes, which a deep
        // OA_SOCKET can exceed; say which path was refused.
        let inner = tokio::net::UnixListener::bind(path)
            .map_err(|e| io::Error::new(e.kind(), format!("{address}: {e}")))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            address: address.into(),
            inner,
        })
    }
    #[cfg(windows)]
    pub fn bind(address: &str) -> io::Result<Self> {
        use tokio::net::windows::named_pipe::ServerOptions;
        let next = ServerOptions::new()
            .first_pipe_instance(true)
            .create(address)
            .map_err(|e| match e.kind() {
                io::ErrorKind::PermissionDenied => in_use(address),
                _ => e,
            })?;
        Ok(Self {
            address: address.into(),
            next,
        })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub async fn accept(&mut self) -> io::Result<Stream> {
        #[cfg(unix)]
        {
            Ok(self.inner.accept().await?.0)
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            self.next.connect().await?;
            let fresh = ServerOptions::new().create(&self.address)?;
            Ok(std::mem::replace(&mut self.next, fresh))
        }
    }
}

#[cfg(unix)]
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.address);
    }
}

fn in_use(address: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AddrInUse,
        format!("another open-analysis window is already serving agents at {address}"),
    )
}

/// Joins this process's stdin and stdout to the GUI's socket, byte for byte,
/// and returns when the GUI closes the connection, or on Windows as soon as
/// the agent closes stdin.
pub async fn bridge(address: &str) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let stream = connect(address).await.map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "no open-analysis GUI is serving agents at {address} ({e}); start the GUI, or run \
                 oa-mcp without --attach for a model of the agent's own"
            ),
        )
    })?;
    let (mut from_gui, mut to_gui) = tokio::io::split(stream);
    let mut stdout = tokio::io::stdout();
    {
        let mut to_agent = std::pin::pin!(tokio::io::copy(&mut from_gui, &mut stdout));
        let from_agent = async {
            let _ = tokio::io::copy(&mut tokio::io::stdin(), &mut to_gui).await;
            let _ = to_gui.shutdown().await;
        };
        tokio::select! {
            copied = &mut to_agent => {
                copied?;
            }
            () = from_agent => {
                // The agent closing stdin ends the session from its side. A
                // Unix socket half-closes, so the GUI sees that, and closes
                // the connection once it has answered, which ends the copy.
                // A named pipe cannot half-close, and the GUI would wait for
                // more input forever, so end here instead: that closes the
                // pipe.
                #[cfg(unix)]
                to_agent.await?;
            }
        }
    }
    stdout.flush().await
}

#[cfg(unix)]
async fn connect(address: &str) -> io::Result<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(address).await
}

#[cfg(windows)]
async fn connect(address: &str) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;
    // ERROR_PIPE_BUSY: every instance is taken for a moment; try again.
    const PIPE_BUSY: i32 = 231;
    for _ in 0..50 {
        match ClientOptions::new().open(address) {
            Err(e) if e.raw_os_error() == Some(PIPE_BUSY) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await
            }
            other => return other,
        }
    }
    ClientOptions::new().open(address)
}
