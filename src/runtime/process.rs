use super::{RpcFault, RpcResult, Runtime};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};
use tokio::{
  io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
  sync::mpsc,
  task::JoinSet,
};

#[derive(PartialEq)]
struct Directories {
  data: PathBuf,
  cache: PathBuf,
}

fn directory(params: &Value, name: &str) -> Result<PathBuf, RpcFault> {
  let value = params[name].as_str().ok_or_else(|| RpcFault::invalid(name))?;
  let path = PathBuf::from(value);
  if !path.is_absolute() || value.contains('\0') {
    return Err(RpcFault::invalid(name));
  }
  Ok(path)
}

fn initialize(
  service: &mut Option<(Directories, Arc<Runtime>)>,
  params: &Value,
) -> Result<Arc<Runtime>, RpcFault> {
  let paths =
    Directories { data: directory(params, "data_dir")?, cache: directory(params, "cache_dir")? };
  if let Some((current, runtime)) = service {
    if *current != paths {
      return Err(RpcFault::new(-32602, "Plugin directories cannot change during a run"));
    }
    return Ok(runtime.clone());
  }
  let runtime = Runtime::with_directories(paths.data.clone(), paths.cache.clone());
  *service = Some((paths, runtime.clone()));
  Ok(runtime)
}

struct Invocation {
  correlation: Option<Value>,
  operation: String,
  arguments: Value,
}

impl Invocation {
  fn decode(line: &str) -> Option<Self> {
    let value: Value = serde_json::from_str(line).ok()?;
    Some(Self {
      correlation: value.get("id").cloned(),
      operation: value.get("method").and_then(Value::as_str).unwrap_or_default().to_owned(),
      arguments: value.get("params").cloned().unwrap_or(Value::Null),
    })
  }

  async fn execute(mut self, runtime: Arc<Runtime>, destination: mpsc::Sender<Value>) {
    let outcome = runtime.call(&self.operation, self.arguments.take()).await;
    self.respond(outcome, destination).await;
  }

  async fn respond(self, outcome: RpcResult, destination: mpsc::Sender<Value>) {
    let Some(correlation) = self.correlation else {
      return;
    };
    let response = match outcome {
      Ok(result) => json!({"jsonrpc":"2.0","id":correlation,"result":result}),
      Err(fault) => {
        let mut error = json!({"code":fault.code,"message":fault.message});
        if let Some(data) = fault.data {
          error["data"] = data;
        }
        json!({"jsonrpc":"2.0","id":correlation,"error":error})
      }
    };
    let _ = destination.send(response).await;
  }
}

pub async fn serve<R, W>(input: R, mut output: W)
where
  R: AsyncRead + Unpin,
  W: AsyncWrite + Send + Unpin + 'static,
{
  let (replies, mut queue) = mpsc::channel::<Value>(64);
  let writer = tokio::spawn(async move {
    while let Some(response) = queue.recv().await {
      let mut bytes = response.to_string().into_bytes();
      bytes.push(b'\n');
      if output.write_all(&bytes).await.is_err() || output.flush().await.is_err() {
        break;
      }
    }
  });
  let mut service = None;
  let mut delivery = None;
  let mut requests = JoinSet::new();
  let mut lines = BufReader::new(input).lines();
  while let Ok(Some(line)) = lines.next_line().await {
    let Some(invocation) = Invocation::decode(&line) else {
      continue;
    };
    if invocation.operation == "initialize" {
      match initialize(&mut service, &invocation.arguments) {
        Ok(runtime) => {
          // Install host permissions and the private desktop channel before background work.
          invocation.execute(runtime.clone(), replies.clone()).await;
          if delivery.is_none() {
            let reporter = runtime.clone();
            delivery = Some(tokio::spawn(async move {
              let mut period = tokio::time::interval(std::time::Duration::from_secs(2));
              loop {
                period.tick().await;
                reporter.flush_reports().await;
              }
            }));
          }
        }
        Err(fault) => invocation.respond(Err(fault), replies.clone()).await,
      }
      continue;
    }
    let Some((_, runtime)) = &service else {
      invocation
        .respond(Err(RpcFault::new(-32002, "Initialize the plugin first")), replies.clone())
        .await;
      continue;
    };
    if invocation.operation == "playback.event" {
      invocation.execute(runtime.clone(), replies.clone()).await;
      continue;
    }
    while requests.len() >= 32 {
      let _ = requests.join_next().await;
    }
    requests.spawn(invocation.execute(runtime.clone(), replies.clone()));
  }
  while requests.join_next().await.is_some() {}
  if let Some(delivery) = delivery {
    delivery.abort();
  }
  drop(replies);
  let _ = writer.await;
}
