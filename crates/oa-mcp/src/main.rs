//! MCP server over stdio.
//!
//! `oa-mcp` serves a model of the agent's own. `oa-mcp --attach` instead
//! joins stdio to the open-analysis GUI, so the agent works on the model
//! open there, in view of the user and on the same undo history.
use oa_mcp::{server::Server, socket};
use rmcp::{ServiceExt, transport::stdio};

const USAGE: &str = "usage: oa-mcp [--attach]

  (no arguments)  serve a model of the agent's own over stdio
  --attach        connect stdio to the model open in the open-analysis GUI

OA_SOCKET sets the GUI's socket path, or pipe name on Windows.";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let attach = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => false,
        ["--attach"] => true,
        ["-h" | "--help"] => {
            println!("{USAGE}");
            return std::process::ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let runtime = tokio::runtime::Runtime::new().expect("start the async runtime");
    let result = runtime.block_on(async {
        if attach {
            let address = socket::address()?.ok_or_else(|| {
                std::io::Error::other("OA_SOCKET is off, so there is no GUI to attach to")
            })?;
            socket::bridge(&address).await?;
        } else {
            let service = Server::headless().serve(stdio()).await?;
            service.waiting().await?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    });
    // Reading stdin holds a blocking thread that would keep the runtime
    // from shutting down once the other side has gone.
    runtime.shutdown_background();
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oa-mcp: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
