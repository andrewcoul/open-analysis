//! An agent attaching over the local socket, end to end: an rmcp client
//! talks to the real `oa-mcp --attach` bridge, which joins it to a listener
//! serving a shared session the way the GUI does. Also, several agents
//! sharing one session.
use oa_mcp::Session;
use oa_mcp::server::{Connection, Host, Server, serve_socket};
use oa_mcp::socket::Listener;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("oa-mcp-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn call(
    client: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool: &'static str,
    arguments: Value,
) -> Value {
    let mut params = CallToolRequestParams::new(tool);
    if let Value::Object(map) = arguments {
        params = params.with_arguments(map);
    }
    let result = client.call_tool(params).await.expect("tool call");
    let result = serde_json::to_value(result).unwrap();
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    serde_json::from_str(text).unwrap_or(Value::String(text.into()))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_attaches_through_the_bridge() {
    #[cfg(unix)]
    let (dir, address) = {
        let dir = scratch("attach");
        let address = dir.join("agent.sock").to_string_lossy().into_owned();
        // A socket file left by a process that died is replaced.
        drop(std::os::unix::net::UnixListener::bind(&address).unwrap());
        assert!(std::path::Path::new(&address).exists());
        (dir, address)
    };
    #[cfg(windows)]
    let address = format!(r"\\.\pipe\oa-mcp-attach-{}", std::process::id());

    let session = Arc::new(Mutex::new(Session::default()));
    let host: Arc<dyn Host> = session.clone();
    let listener = Listener::bind(&address).unwrap();
    // A second window cannot take the address while the first serves it.
    let taken = Listener::bind(&address).err().unwrap();
    assert_eq!(taken.kind(), std::io::ErrorKind::AddrInUse);
    let (events, mut heard) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(serve_socket(listener, host, move |c| {
        let _ = events.send(c);
    }));

    let mut bridge = tokio::process::Command::new(env!("CARGO_BIN_EXE_oa-mcp"))
        .arg("--attach")
        .env("OA_SOCKET", &address)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let transport = (bridge.stdout.take().unwrap(), bridge.stdin.take().unwrap());
    let client = ().serve(transport).await.expect("handshake through the bridge");

    let tools: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    for tool in [
        "generate_combinations",
        "modal",
        "response_spectrum",
        "spectrum_peaks",
    ] {
        assert!(
            tools.iter().any(|t| t == tool),
            "{tool} missing from {tools:?}"
        );
    }

    let described = call(&client, "describe_model", json!({})).await;
    assert_eq!(described["counts"]["levels"], 1);
    let id = call(&client, "next_ids", json!({"count": 1})).await["ids"][0].clone();
    let applied = call(
        &client,
        "apply_commands",
        json!({"commands": [{"command": "add_level", "id": id, "level": {"name": "L2", "elevation": 12}}]}),
    )
    .await;
    assert_eq!(applied["applied"], 1, "{applied}");
    // The edit landed in the shared session, where the GUI would see it.
    assert_eq!(session.lock().unwrap().model().levels.len(), 2);
    assert_eq!(call(&client, "undo", json!({})).await["undone"], true);
    assert_eq!(session.lock().unwrap().model().levels.len(), 1);

    // The agent leaving ends the bridge and the GUI's side of the
    // connection, on Windows too, where a named pipe cannot half-close.
    client.cancel().await.unwrap();
    let status = tokio::time::timeout(std::time::Duration::from_secs(10), bridge.wait())
        .await
        .expect("the bridge exits once the agent has gone")
        .unwrap();
    assert!(status.success(), "bridge exited with {status}");
    assert_eq!(heard.recv().await, Some(Connection::Opened));
    assert_eq!(heard.recv().await, Some(Connection::Closed));
    #[cfg(unix)]
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn attached_agents_undo_only_their_own_changes() {
    let session = Arc::new(Mutex::new(Session::default()));
    let host: Arc<dyn Host> = session.clone();
    let mut agents = vec![];
    for _ in 0..2 {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let server = Server::new(host.clone());
        tokio::spawn(async move {
            if let Ok(service) = server.serve(theirs).await {
                let _ = service.waiting().await;
            }
        });
        agents.push(().serve(ours).await.expect("handshake"));
    }
    let named =
        |name: &str| json!({"commands": [{"command": "set_metadata", "metadata": {"name": name}}]});
    call(&agents[0], "apply_commands", named("first")).await;
    call(&agents[1], "apply_commands", named("second")).await;

    let refused = agents[0]
        .call_tool(CallToolRequestParams::new("undo"))
        .await
        .expect_err("the first agent's undo would take the second's change");
    assert!(refused.to_string().contains("not yours"), "{refused}");
    assert_eq!(session.lock().unwrap().model().metadata.name, "second");
    assert_eq!(call(&agents[1], "undo", json!({})).await["undone"], true);
    assert_eq!(session.lock().unwrap().model().metadata.name, "first");
}

#[test]
fn the_bridge_explains_when_no_gui_is_running() {
    let dir = scratch("absent");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_oa-mcp"))
        .arg("--attach")
        .env("OA_SOCKET", dir.join("nobody.sock"))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no open-analysis GUI"), "{stderr}");
    std::fs::remove_dir_all(&dir).unwrap();
}
