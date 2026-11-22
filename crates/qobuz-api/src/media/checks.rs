use super::*;
use tokio::{
  io::{AsyncReadExt, AsyncWriteExt},
  net::TcpListener,
  sync::{mpsc, oneshot, Notify},
};

fn packet(byte: u8) -> Vec<u8> {
  let mut data = 44u32.to_be_bytes().to_vec();
  data.extend_from_slice(b"uuid");
  data.extend_from_slice(&[59, 66, 18, 146, 86, 243, 95, 117, 146, 54, 99, 182, 154, 31, 82, 178]);
  data.extend_from_slice(&[0; 4]);
  data.extend_from_slice(&52u32.to_be_bytes());
  data.extend_from_slice(&[0, 0, 0, 1]);
  data.extend_from_slice(&1u32.to_be_bytes());
  data.extend_from_slice(&[0; 4]);
  data.extend_from_slice(&9u32.to_be_bytes());
  data.extend_from_slice(b"mdat");
  data.push(byte);
  data
}

#[tokio::test]
async fn test_ordered_window_and_cancellation() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  for cancel in [false, true] {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let template = format!("http://{}/$SEGMENT$", listener.local_addr().unwrap());
    let release = Arc::new(Notify::new());
    let signal = release.clone();
    let (observed, mut incoming) = mpsc::unbounded_channel();
    let (closed, closure) = oneshot::channel();
    let closed = Arc::new(std::sync::Mutex::new(Some(closed)));
    let server = tokio::spawn(async move {
      let mut children = tokio::task::JoinSet::new();
      loop {
        let (mut socket, _) = listener.accept().await.unwrap();
        let signal = signal.clone();
        let observed = observed.clone();
        let closed = closed.clone();
        children.spawn(async move {
          let mut bytes = [0;4096]; let size = socket.read(&mut bytes).await.unwrap();
          let request = String::from_utf8_lossy(&bytes[..size]);
          let number: u8 = request.split_whitespace().nth(1).unwrap().trim_start_matches('/').parse().unwrap();
          observed.send(number).unwrap();
          if number == 1 {
            let mut probe = [0;1];
            tokio::select! {
              _ = signal.notified() => (),
              result = socket.read(&mut probe) => {
                if let Some(closed) = closed.lock().unwrap().take() { let _ = closed.send(result.unwrap_or(0) == 0); }
                return;
              }
            }
          }
          let body = packet(b'A' + number - 1);
          let header = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
          socket.write_all(header.as_bytes()).await.unwrap(); socket.write_all(&body).await.unwrap();
        });
      }
    });
    let plan = DeliveryPlan {
      index: AudioIndex {
        preamble: b"header".to_vec(),
        chunks: vec![media_core::ChunkSize { bytes: 1, samples: 1 }; 6],
      },
      key: ContentKey::from_bytes([0; 16]),
      quality: StreamQualityInfo::from_raw(6, Some(44.1), Some(16)),
      init_latency_ms: 0,
      template,
      count: 6,
      descriptor: String::new(),
      original_init: vec![],
      source: SegmentSource::new().unwrap(),
    };
    let download = tokio::spawn(async move { plan.collect(None).await });
    let mut first = vec![];
    for _ in 0..REQUEST_WINDOW {
      first.push(
        tokio::time::timeout(Duration::from_secs(2), incoming.recv()).await.unwrap().unwrap(),
      );
    }
    first.sort();
    assert_eq!(first, [1, 2, 3]);
    assert!(tokio::time::timeout(Duration::from_millis(650), incoming.recv()).await.is_err());
    if cancel {
      download.abort();
      let _ = download.await;
      // No later request may enter the window after its owning transfer is dropped.
      assert!(tokio::time::timeout(Duration::from_millis(100), incoming.recv()).await.is_err());
      assert!(tokio::time::timeout(Duration::from_secs(2), closure).await.unwrap().unwrap());
    } else {
      release.notify_one();
      assert_eq!(download.await.unwrap().unwrap(), b"headerABCDEF");
    }
    server.abort();
    let _ = server.await;
  }
}

#[tokio::test]
async fn test_body_timeout_retries_and_terminal_status() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = format!("http://{}/fixture?token=private-fixture", listener.local_addr().unwrap());
  let server = tokio::spawn(async move {
    let (mut stalled, _) = listener.accept().await.unwrap();
    let mut bytes = [0; 4096];
    stalled.read(&mut bytes).await.unwrap();
    stalled
      .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n")
      .await
      .unwrap();
    let (mut retry, _) = listener.accept().await.unwrap();
    retry.read(&mut bytes).await.unwrap();
    retry
      .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndone")
      .await
      .unwrap();
  });
  let mut source = SegmentSource::new().unwrap();
  source.deadline = Duration::from_millis(40);
  source.attempts = 2;
  assert_eq!(source.read(&address).await.unwrap(), b"done");
  server.await.unwrap();
  let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
  let address = format!("http://{}/fixture?token=private-fixture", listener.local_addr().unwrap());
  let server = tokio::spawn(async move {
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut bytes = [0; 4096];
    socket.read(&mut bytes).await.unwrap();
    socket
      .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
      .await
      .unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(500), listener.accept()).await.is_err());
  });
  let error = source.read(&address).await.unwrap_err();
  assert!(error.contains("403"));
  assert!(!error.contains("private-fixture"));
  server.await.unwrap();
}
