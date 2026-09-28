//! Lets an AI agent work on the open model. The MCP tools are served on a
//! local socket that `oa-mcp --attach` joins to the agent's stdio. Calls
//! arrive on a background thread and run here, on the GUI thread, against
//! the document's session, so they take turns with the person's own edits
//! and share their undo history. The design is in `docs/mcp/PLAN.md`.
use crate::document::Document;
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use gpui_kit::{App, Entity};
use oa_mcp::server::{Connection, Host, Job, Reply, serve_socket};
use serde_json::Value;
use std::sync::Arc;

/// Whether an agent can reach the open model, for the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AgentStatus {
    #[default]
    Starting,
    /// Not serving agents, and why.
    Off(String),
    /// Serving at this address, to this many connected agents.
    Ready { address: String, connected: usize },
}

impl AgentStatus {
    /// The status bar's label and its hover text.
    pub fn describe(&self) -> (String, String) {
        match self {
            Self::Starting => ("Agent starting".into(), "Opening the agent socket".into()),
            Self::Off(why) => ("Agent off".into(), format!("Agents cannot attach: {why}")),
            Self::Ready { connected: 0, .. } => (
                "Agent ready".into(),
                "An MCP client can attach with the command oa-mcp --attach".into(),
            ),
            Self::Ready { connected, address } => (
                if *connected == 1 {
                    "Agent attached".into()
                } else {
                    format!("{connected} agents attached")
                },
                format!("Edits from agents at {address} show here and undo with Ctrl+Z"),
            ),
        }
    }
}

const ATTACHED: &str = "You are attached to the open-analysis desktop GUI. The model is the one \
    open on the user's screen: they see each edit as you make it, analyses you run are drawn in \
    their view, and you share one undo history with them. undo takes back only your own last \
    change, and new_model and load_model are refused while they have unsaved work.";

enum Message {
    Call(Job, oneshot::Sender<Result<Value, String>>),
    Ready(String),
    Off(String),
    Connection(Connection),
}

/// Hands each tool call to the GUI thread and waits for its reply.
struct GuiHost(mpsc::UnboundedSender<Message>);

impl Host for GuiHost {
    fn run(&self, job: Job) -> Reply {
        let (reply, answer) = oneshot::channel();
        let sent = self.0.unbounded_send(Message::Call(job, reply));
        Box::pin(async move {
            sent.map_err(|_| "the open-analysis window has closed".to_string())?;
            answer
                .await
                .map_err(|_| "the open-analysis window dropped the call".to_string())?
        })
    }
    fn instructions(&self) -> Option<&'static str> {
        Some(ATTACHED)
    }
}

/// Starts serving agents on the document. The socket lives on a thread of
/// its own; everything it asks of the document comes back here.
pub fn start(document: Entity<Document>, cx: &mut App) {
    let address = match oa_mcp::socket::address() {
        Ok(Some(address)) => address,
        Ok(None) => return off(&document, "OA_SOCKET is set to off".into(), cx),
        Err(e) => return off(&document, e.to_string(), cx),
    };
    let (sender, mut messages) = mpsc::unbounded();
    let host: Arc<dyn Host> = Arc::new(GuiHost(sender.clone()));
    let spawned = std::thread::Builder::new()
        .name("agent socket".into())
        .spawn(move || serve(address, host, sender));
    if let Err(e) = spawned {
        return off(&document, e.to_string(), cx);
    }
    let document = document.downgrade();
    cx.spawn(async move |cx| {
        while let Some(message) = messages.next().await {
            let handled = document.update(cx, |document, cx| match message {
                Message::Call(job, reply) => {
                    let _ = reply.send(document.run_agent_job(job, cx));
                }
                Message::Ready(address) => document.set_agent(
                    AgentStatus::Ready {
                        address,
                        connected: 0,
                    },
                    cx,
                ),
                Message::Off(why) => {
                    eprintln!("oa-gui: agents cannot attach: {why}");
                    document.set_agent(AgentStatus::Off(why), cx)
                }
                Message::Connection(change) => {
                    if let AgentStatus::Ready { address, connected } = document.agent().clone() {
                        let connected = match change {
                            Connection::Opened => connected + 1,
                            Connection::Closed => connected.saturating_sub(1),
                        };
                        document.set_agent(AgentStatus::Ready { address, connected }, cx);
                    }
                }
            });
            if handled.is_err() {
                break;
            }
        }
    })
    .detach();
}

fn off(document: &Entity<Document>, why: String, cx: &mut App) {
    eprintln!("oa-gui: agents cannot attach: {why}");
    document.update(cx, |document, cx| {
        document.set_agent(AgentStatus::Off(why), cx)
    });
}

/// The socket thread: binds the address and serves every agent that
/// connects until the process ends.
fn serve(address: String, host: Arc<dyn Host>, sender: mpsc::UnboundedSender<Message>) {
    let say = |message| {
        let _ = sender.unbounded_send(message);
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => return say(Message::Off(e.to_string())),
    };
    runtime.block_on(async {
        let listener = match oa_mcp::socket::Listener::bind(&address) {
            Ok(listener) => listener,
            Err(e) => return say(Message::Off(e.to_string())),
        };
        say(Message::Ready(address));
        let events = sender.clone();
        let served = serve_socket(listener, host, move |change| {
            let _ = events.unbounded_send(Message::Connection(change));
        })
        .await;
        if let Err(e) = served {
            say(Message::Off(e.to_string()));
        }
    });
}
