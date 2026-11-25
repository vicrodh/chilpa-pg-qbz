use super::*;
use tokio::{
  io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
  net::TcpListener,
};

struct BrowserFixture {
  endpoint: Value,
  worker: tokio::task::JoinHandle<()>,
  state: std::sync::Arc<std::sync::Mutex<(String, bool, bool)>>,
}
impl BrowserFixture {
  async fn confirmed() -> Self {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint =
      json!({"protocol":1,"port":listener.local_addr().unwrap().port(),"key":"x".repeat(43)});
    let state = std::sync::Arc::new(std::sync::Mutex::new(("verify".to_string(), false, false)));
    let shared = state.clone();
    let worker = tokio::spawn(async move {
      loop {
        let (socket, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(socket);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let session = line.starts_with("POST /session ");
        let mut length = 0;
        loop {
          line.clear();
          reader.read_line(&mut line).await.unwrap();
          if line == "\r\n" {
            break;
          }
          if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
          }
        }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes).await.unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        let (phase, invalid, snap) = {
          let mut state = shared.lock().unwrap();
          if request["action"] == "verified" {
            state.0 = "ready".into();
          }
          if request["action"] == "disconnect" {
            state.0 = "login".into();
          }
          state.clone()
        };
        let reply = if session {
          json!({"app_id":"123456789","session":{"user_auth_token":"fixture-token","user_id":if invalid {0} else {42},"email":"","display_name":"Fixture","subscription_label":"","entitlements":{},"country_code":null,"language_code":null}})
        } else {
          json!({"phase":phase,"found":[],"mode":"native","snap":snap})
        };
        let body = reply.to_string();
        reader.get_mut().write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
      }
    });
    Self { endpoint, worker, state }
  }
}
impl Drop for BrowserFixture {
  fn drop(&mut self) {
    self.worker.abort();
  }
}

#[tokio::test]
async fn confirmed_browser_session_is_usable_without_a_second_manual_check() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let fixture = BrowserFixture::confirmed().await;
  let directory = tempfile::tempdir().unwrap();
  let access = AccessController::new(directory.path());
  access.configure(fixture.endpoint.clone());
  let api = QobuzClient::new().unwrap();
  access.load(&api).await.unwrap();
  access.refresh(&api).await.unwrap();
  assert!(api.is_logged_in().await);
  let first = access.form(&api, "es").await;
  assert_eq!(first["status"]["phase"], "ready");
  assert!(!first.to_string().contains("fixture-token"));
  let second = access.form(&api, "es").await;
  assert_eq!(first["revision"], second["revision"]);
}

#[tokio::test]
async fn returning_to_settings_activates_the_confirmed_browser_account() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let fixture = BrowserFixture::confirmed().await;
  fixture.state.lock().unwrap().0 = "login".into();
  let directory = tempfile::tempdir().unwrap();
  let access = AccessController::new(directory.path());
  access.configure(fixture.endpoint.clone());
  let api = QobuzClient::new().unwrap();
  access.load(&api).await.unwrap();
  assert!(!api.is_logged_in().await);
  fixture.state.lock().unwrap().0 = "verify".into();
  let form = access.form(&api, "es").await;
  assert_eq!(form["status"]["phase"], "ready");
  assert!(form["wizard"]["description"].as_str().unwrap().contains("buscar y reproducir"));
  assert_eq!(fixture.state.lock().unwrap().0, "ready");
  access.sign_out(&api).await.unwrap();
  assert!(!api.is_logged_in().await);
  assert_eq!(fixture.state.lock().unwrap().0, "login");
}

#[tokio::test]
async fn invalid_browser_credentials_never_mark_the_plugin_ready() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let fixture = BrowserFixture::confirmed().await;
  fixture.state.lock().unwrap().1 = true;
  let directory = tempfile::tempdir().unwrap();
  let access = AccessController::new(directory.path());
  access.configure(fixture.endpoint.clone());
  let api = QobuzClient::new().unwrap();
  access.load(&api).await.unwrap();
  let form = access.form(&api, "en").await;
  assert_ne!(form["status"]["phase"], "ready");
  assert!(!form["notices"].as_array().unwrap().is_empty());
  assert!(!api.is_logged_in().await);
  assert_eq!(fixture.state.lock().unwrap().0, "verify");
}

#[tokio::test]
async fn catalog_access_can_complete_login_after_the_wizard_is_closed() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let fixture = BrowserFixture::confirmed().await;
  fixture.state.lock().unwrap().0 = "waiting".into();
  let directory = tempfile::tempdir().unwrap();
  let session =
    crate::runtime::SessionContext::new(directory.path().into(), directory.path().into());
  session.access.configure(fixture.endpoint.clone());
  session.load().await.unwrap();
  assert!(session.require_identity().await.is_err());
  fixture.state.lock().unwrap().0 = "verify".into();
  session.require_identity().await.unwrap();
  assert!(session.api.is_logged_in().await);
}

#[test]
fn desktop_endpoint_validation_rejects_privileged_ports_and_invalid_keys() {
  for (port, key, valid) in [
    (48639, "x".repeat(43), true),
    (80, "x".repeat(43), false),
    (48639, "short".into(), false),
    (48639, "x ".repeat(32), false),
  ] {
    assert_eq!(BrowserConnection { port, key }.validate().is_ok(), valid);
  }
}

async fn described(host: Option<Value>) -> Value {
  let directory = tempfile::tempdir().unwrap();
  let access = AccessController::new(directory.path());
  if let Some(host) = host {
    access.configure(host);
  }
  access.form(&QobuzClient::new().unwrap(), "en").await
}

#[tokio::test]
async fn each_reason_the_helper_is_missing_has_its_own_message() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  for (host, expected) in [
    (Some(json!({"error":"network_disabled"})), "Offline mode"),
    (Some(json!({"error":"desktop_start"})), "startup.log"),
    (Some(json!({"error":"desktop_something_new"})), "(desktop_something_new)"),
    (None, "Update Chilpa"),
  ] {
    let form = described(host.clone()).await;
    let description = form["wizard"]["description"].as_str().unwrap();
    assert!(description.contains(expected), "{host:?}: {description}");
    assert_eq!(form["wizard"]["step"], 1);
  }
}

#[tokio::test]
async fn a_helper_that_stopped_answering_says_so_with_its_reason() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
  let form = described(Some(json!({"protocol":1,"port":port,"key":"x".repeat(43)}))).await;
  assert!(form["wizard"]["description"].as_str().unwrap().contains("does not answer"));
  assert!(!form["notices"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn settings_opened_before_the_startup_load_reach_the_helper() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let fixture = BrowserFixture::confirmed().await;
  fixture.state.lock().unwrap().0 = "login".into();
  let form = described(Some(fixture.endpoint.clone())).await;
  assert_eq!(form["wizard"]["step"], 2);
  assert!(form["wizard"]["description"].as_str().unwrap().starts_with("Firefox detected"));
}

#[tokio::test]
async fn snap_firefox_says_where_its_profile_lives() {
  let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
  let fixture = BrowserFixture::confirmed().await;
  *fixture.state.lock().unwrap() = ("login".into(), false, true);
  let form = described(Some(fixture.endpoint.clone())).await;
  assert!(form["wizard"]["description"].as_str().unwrap().contains("~/snap/firefox/common"));
}
