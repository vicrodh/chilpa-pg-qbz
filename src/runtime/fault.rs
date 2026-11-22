use qobuz_api::ApiError;
use serde_json::{json, Value};

pub type RpcResult = Result<Value, RpcFault>;

#[derive(Debug)]
pub struct RpcFault {
  pub code: i64,
  pub message: String,
  pub data: Option<Value>,
}

impl RpcFault {
  pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
    Self { code, message: message.into(), data: None }
  }
  pub(crate) fn invalid(subject: &str) -> Self {
    Self::new(-32602, format!("invalid {subject}"))
  }
  pub(crate) fn busy(message: impl Into<String>, seconds: Option<u64>) -> Self {
    Self {
      code: -32000,
      message: message.into(),
      data: Some(json!({"retry_after_ms": seconds.unwrap_or(1).clamp(1, 3600) * 1000})),
    }
  }
}

impl From<ApiError> for RpcFault {
  fn from(fault: ApiError) -> Self {
    let message = fault.to_string();
    match fault {
      ApiError::AuthenticationError(_)
      | ApiError::InvalidAppId
      | ApiError::InvalidAppSecret
      | ApiError::IneligibleUser
      | ApiError::Forbidden(_)
      | ApiError::ForbiddenCircuitOpen(_) => Self::new(-32001, message),
      ApiError::RateLimited(seconds) => Self::busy(message, Some(seconds)),
      ApiError::OfflineMode => Self { code: -32000, message, data: Some(json!({"terminal":true})) },
      _ => Self::new(-32000, message),
    }
  }
}
