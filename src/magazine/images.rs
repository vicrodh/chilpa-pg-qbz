use super::Value;

const ORIGIN: &str = "https://qobuz-qobuz-prod.web.arc-cdn.net";

pub(super) fn public_image(value: &Value) -> Option<String> {
  sized_image(value, 1600)
}

/// Arc's per-image public signature is distinct from account authentication.
/// Restrict the derivative to the known resizer, preserving the whole image.
pub(super) fn sized_image(value: &Value, maximum: u64) -> Option<String> {
  let derivative = value
    .pointer("/additional_properties/fullSizeResizeUrl")
    .or_else(|| value.pointer("/additional_properties/resizeUrl"))
    .or_else(|| value.get("url"))
    .and_then(Value::as_str);
  if let Some(address) = derivative.filter(|address| address.len() <= 4096) {
    let address = if address.starts_with("/resizer/v2/") {
      format!("{ORIGIN}{address}")
    } else {
      address.into()
    };
    if let Ok(url) = url::Url::parse(&address) {
      if url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && matches!(
          url.host_str(),
          Some("qobuz-qobuz-prod.web.arc-cdn.net" | "qobuz-qobuz-sandbox.cdn.arcpublishing.com")
        )
      {
        if let Some(name) = url.path().strip_prefix("/resizer/v2/") {
          if let Some((id, extension)) = name.rsplit_once('.') {
            if !id.is_empty()
              && id.len() <= 128
              && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
              && matches!(extension, "jpg" | "jpeg" | "png" | "webp" | "avif")
            {
              let signature = url
                .query_pairs()
                .find(|(key, _)| key == "auth")
                .map(|(_, value)| value.into_owned())?;
              if signature.len() != 64 || !signature.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
              }
              let width = value["width"].as_u64().filter(|size| (1..=50000).contains(size))?;
              let height = value["height"].as_u64().filter(|size| (1..=50000).contains(size))?;
              let maximum = maximum.clamp(1, 1600).min(width.max(height));
              let target = (width * maximum / width.max(height)).max(1);
              let mut result = url::Url::parse(&format!("{ORIGIN}/resizer/v2/{name}")).ok()?;
              result
                .query_pairs_mut()
                .append_pair("auth", &signature)
                .append_pair("width", &target.to_string())
                .append_pair("quality", "75");
              return Some(result.into());
            }
          }
        }
      }
    }
  }
  let address =
    value.pointer("/additional_properties/originalUrl").or_else(|| value.get("url"))?.as_str()?;
  if address.len() > 4096 {
    return None;
  }
  let url = url::Url::parse(address).ok()?;
  let cover = url.path().strip_prefix("/images/covers/")?;
  let size = match cover.rsplit_once('_')?.1 {
    "50.jpg" => 50,
    "230.jpg" => 230,
    "600.jpg" => 600,
    _ => return None,
  };
  (url.scheme() == "https"
    && url.host_str() == Some("static.qobuz.com")
    && url.username().is_empty()
    && url.password().is_none()
    && url.port_or_known_default() == Some(443)
    && url.query().is_none()
    && url.fragment().is_none()
    && size <= maximum.clamp(1, 1600))
  .then(|| address.to_owned())
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn portrait_derivatives_fit_the_bound_without_cropping_or_forwarding_extra_fields() {
    let value = json!({"width":5749,"height":9033,"additional_properties":{
      "fullSizeResizeUrl":format!("/resizer/v2/IMAGE.jpeg?auth={}&token=private&width=9000","a".repeat(64))}});
    let result = public_image(&value).unwrap();
    assert!(result.starts_with(ORIGIN));
    assert!(result.contains("width=1018"));
    assert!(!result.contains("private"));
    assert!(!result.contains("9000"));
    assert!(sized_image(&value, 600).unwrap().contains("width=381"));
  }

  #[test]
  fn originals_without_a_bounded_derivative_are_not_published() {
    assert!(public_image(&json!({"url":"https://cloudfront-eu-central-1.images.arcpublishing.com/qobuz/a.jpg","width":9000,"height":9000})).is_none());
    for address in [
      "https://evil.test/resizer/v2/A.jpg?auth=bad",
      "//evil.test/resizer/v2/A.jpg?auth=bad",
      "/resizer/v2/../A.jpg?auth=bad",
    ] {
      assert!(public_image(&json!({"url":address,"width":1000,"height":1000})).is_none());
    }
  }

  #[test]
  fn static_images_require_a_known_cover_size_within_the_requested_bound() {
    let cover = json!({"url":"https://static.qobuz.com/images/covers/4j/vw/nzxh5konmvw4j_600.jpg"});
    assert!(sized_image(&cover, 600).is_some());
    assert!(sized_image(&cover, 230).is_none());
    for address in [
      "https://static.qobuz.com/images/labels/backgrounds/label.jpg",
      "https://static.qobuz.com/images/covers/album_3000.jpg",
      "https://static.qobuz.com/images/covers/album_600.jpg#original",
    ] {
      assert!(public_image(&json!({"url":address,"width":4000,"height":4000})).is_none());
    }
  }
}
