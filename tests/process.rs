use pg_qbz::runtime::process;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn test_process_framing_notifications_and_eof() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let profile = tempfile::tempdir().unwrap();
  let (mut input, reader) = tokio::io::duplex(8192);
  let (writer, output) = tokio::io::duplex(8192);
  let service = tokio::spawn(process::serve(reader, writer));
  input.write_all(b"{broken\n").await.unwrap();
  let requests = [
    json!({"id":7,"method":"initialize","params":{"playback_events":1,"data_dir":profile.path(),"cache_dir":profile.path()}}),
    json!({"method":"inspect"}),
    json!({"id":null,"method":"inspect"}),
    json!({"id":"lookup","method":"missing"}),
    json!({"method":"playback.event","params":{"kind":"started"}}),
    json!({"id":8,"method":"reporting.status"}),
  ];
  for request in requests {
    input.write_all(format!("{request}\n").as_bytes()).await.unwrap();
  }
  input.shutdown().await.unwrap();
  let mut lines = BufReader::new(output).lines();
  let mut responses = vec![];
  while let Some(line) = lines.next_line().await.unwrap() {
    responses.push(serde_json::from_str::<Value>(&line).unwrap());
  }
  service.await.unwrap();
  assert_eq!(responses.len(), 4);
  let lookup = |id: Value| responses.iter().find(|response| response["id"] == id).unwrap();
  assert_eq!(lookup(Value::Null)["result"], json!({"settings_pages":["account"]}));
  assert_eq!(lookup(json!("lookup"))["error"]["code"], -32601);
  assert_eq!(lookup(json!(7))["result"]["protocol"], 1);
  assert_eq!(lookup(json!(8))["result"]["queued"], 0);
}

async fn exchange(requests: Vec<Value>) -> Vec<Value> {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let (mut input, reader) = tokio::io::duplex(8192);
  let (writer, output) = tokio::io::duplex(8192);
  let service = tokio::spawn(process::serve(reader, writer));
  for request in requests {
    input.write_all(format!("{request}\n").as_bytes()).await.unwrap();
  }
  input.shutdown().await.unwrap();
  let mut responses = Vec::new();
  let mut lines = BufReader::new(output).lines();
  while let Some(line) = lines.next_line().await.unwrap() {
    responses.push(serde_json::from_str(&line).unwrap());
  }
  service.await.unwrap();
  responses
}

#[tokio::test]
async fn initialization_requires_both_absolute_directories_before_state_access() {
  let profile = tempfile::tempdir().unwrap();
  let data = profile.path().join("private data");
  let cache = profile.path().join("cache");
  let requests = vec![
    json!({"id":1,"method":"auth.logout"}),
    json!({"id":2,"method":"initialize","params":{}}),
    json!({"id":3,"method":"initialize","params":{"data_dir":"relative","cache_dir":cache}}),
    json!({"id":4,"method":"initialize","params":{"data_dir":data,"cache_dir":""}}),
    json!({"id":5,"method":"initialize","params":{"data_dir":data,"cache_dir":42}}),
    json!({"id":6,"method":"initialize","params":{"data_dir":"/invalid\u{0}path","cache_dir":cache}}),
  ];
  let replies = exchange(requests).await;
  assert_eq!(replies.len(), 6);
  assert_eq!(replies[0]["error"]["code"], -32002);
  assert!(replies[1..].iter().all(|reply| reply["error"]["code"] == -32602));
  assert!(!data.exists());
  assert!(!cache.exists());
}

#[tokio::test]
async fn initialized_directories_survive_renegotiation_and_restart() {
  let profile = tempfile::tempdir().unwrap();
  let data = profile.path().join("private data");
  let cache = profile.path().join("cache");
  std::fs::create_dir(&data).unwrap();
  std::fs::create_dir(&cache).unwrap();
  let params = json!({"data_dir":data,"cache_dir":cache,"locale":"en"});
  let replies = exchange(vec![
    json!({"id":1,"method":"initialize","params":params}),
    json!({"id":2,"method":"initialize","params":{"data_dir":data,"cache_dir":cache,"playback_events":1}}),
    json!({"id":3,"method":"initialize","params":{"data_dir":data,"cache_dir":profile.path().join("other")}}),
    json!({"id":4,"method":"auth.logout"}),
  ]).await;
  assert_eq!(replies[0]["result"]["protocol"], 1);
  assert_eq!(replies[1]["result"]["protocol"], 1);
  assert_eq!(replies[2]["error"]["code"], -32602);
  assert_eq!(replies[3]["result"]["signed_in"], false);
  assert!(!data.join("account.json").exists());
  assert!(!cache.join("account.json").exists());
  let restarted = exchange(vec![
    json!({"id":1,"method":"initialize","params":params}),
    json!({"id":2,"method":"auth.status"}),
  ])
  .await;
  assert_eq!(restarted[1]["result"]["signed_in"], false);
  assert!(!data.join("account.json").exists());
}
