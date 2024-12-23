use std::process::Stdio;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn test_e2e() {
  // Run cargo run --bin server and send init_msg to it via stdout, and then read the response from stdin
  println!("Running server");
  let mut server_process = tokio::process::Command::new("cargo")
    .args(["run", "--bin", "server"])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .spawn()
    .unwrap();

  let mut server_stdin = server_process.stdin.take().unwrap();
  let server_stdout = server_process.stdout.take().unwrap();
  let mut server_reader = BufReader::new(server_stdout);

  // Send initialize request
  let init_request = json!({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {
        "roots": {
          "listChanged": true
        },
        "sampling": {}
      },
      "clientInfo": {
        "name": "ExampleClient",
        "version": "1.0.0"
      }
    }
  });

  let init_msg = serde_json::to_string(&init_request).unwrap() + "\n";
  println!("Sending init message: {}", init_msg);
  server_stdin.write_all(init_msg.as_bytes()).await.unwrap();
  server_stdin.flush().await.unwrap();
  server_stdin.shutdown().await.unwrap();
  drop(server_stdin);

  let mut lines = vec![];
  let mut line = String::new();
  while let Ok(n) = server_reader.read_line(&mut line).await {
    if n == 0 {
      break;
    }
    println!("Received: {}", line);
    lines.push(line.trim().to_string());
    line.clear();

    if lines.len() == 1 {
      break;
    }
  }

  // Should be something like this
  assert_eq!(lines, vec![
    r#"
{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"logging":{},"prompts":{"listChanged":false},"resources":{"listChanged":false,"subscribe":false},"tools":{"listChanged":false}},"name":"test-server","protocolVersion":"1.0","version":"1.0.0"},"error":null}
    "#.trim(),
  ]);
}
