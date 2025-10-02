use axum;
use bytes::Bytes;
use orion_error::Error;
use orion_lib::PolyBody;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

pub fn must_traverse<'a, T>(value: &'a Value, path: &[&str], f: impl Fn(&'a Value) -> Option<T>) -> anyhow::Result<T> {
    if let Some(res) = traverse(value, path).and_then(f) {
        Ok(res)
    } else {
        Err(anyhow::anyhow!("missing field {}", path.join(".")))
    }
}

pub fn traverse<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(value);
    }
    path.iter().try_fold(value, |target, token| match target {
        Value::Object(map) => map.get(*token),
        Value::Array(list) => parse_index(token).and_then(|x| list.get(x)),
        _ => None,
    })
}

pub fn traverse_mut<'a>(value: &'a mut Value, path: &[&str]) -> Option<&'a mut Value> {
    if path.is_empty() {
        return Some(value);
    }
    path.iter().try_fold(value, |target, token| match target {
        Value::Object(map) => map.get_mut(*token),
        Value::Array(list) => parse_index(token).and_then(|x| list.get_mut(x)),
        _ => None,
    })
}

fn parse_index(s: &str) -> Option<usize> {
    if s.starts_with('+') || (s.starts_with('0') && s.len() != 1) {
        return None;
    }
    s.parse().ok()
}

pub async fn from_body<T: DeserializeOwned>(body: PolyBody) -> Result<T, Error> {
    // TODO: configurable limit
    let bytes = to_bytes(body, 2_097_152).await?;
    // Try to parse the response body as JSON
    let t = serde_json::from_slice::<T>(bytes.as_ref())?;
    Ok(t)
}

pub async fn inspect_body<T: DeserializeOwned>(body: &mut PolyBody) -> Result<T, Error> {
    let orig = std::mem::replace(body, PolyBody::Empty(http_body_util::Empty::new()));
    //
    let bytes = to_bytes(orig, 2_097_152).await?;
    // Try to parse the response body as JSON
    let t = serde_json::from_slice::<T>(bytes.as_ref());
    // Regardless of an error or not, we should reset the body back
    *body = PolyBody::Full(http_body_util::Full::from(bytes));
    t.map_err(Into::into)
}

pub fn to_body<T: Serialize>(j: T) -> Result<PolyBody, Error> {
    let bytes = serde_json::to_vec(&j)?;
    Ok(PolyBody::from(bytes))
}

pub async fn to_bytes(body: PolyBody, limit: usize) -> Result<Bytes, Error> {
    use http_body_util::BodyExt;
    let body: axum::body::Body = match body {
        PolyBody::Empty(_) => return Err("Can't convert empty".into()),
        PolyBody::Full(full) => axum::body::Body::from_stream(full.into_data_stream()),
        PolyBody::Incoming(incoming) => axum::body::Body::from_stream(incoming.into_data_stream()),
        PolyBody::Timeout(_) => return Err("Can't convert timeout".into()),
        PolyBody::Grpc(_) => return Err("Can't convert grpc".into()),
    };
    axum::body::to_bytes(body, limit).await.map_err(|e| Error::new(e.to_string()))
}
