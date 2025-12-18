use bytes::Bytes;
use http::{uri, HeaderName, HeaderValue, Request, Uri};
use http_body_util::{BodyExt, Limited};
use serde::{Serialize, Serializer};

pub use ::http::uri::{Authority, Scheme};

use crate::PolyBody;

#[derive(Debug, Clone)]
pub struct BufferLimit(pub usize);

impl BufferLimit {
    pub fn new(limit: usize) -> Self {
        BufferLimit(limit)
    }
}

pub fn buffer_limit(req: &Request<PolyBody>) -> usize {
    req.extensions().get::<BufferLimit>().map(|b| b.0).unwrap_or(2_097_152)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HeaderOrPseudo {
    Header(HeaderName),
    Method,
    Scheme,
    Authority,
    Path,
    Status,
}

impl TryFrom<&str> for HeaderOrPseudo {
    type Error = ::http::header::InvalidHeaderName;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            ":method" => Ok(HeaderOrPseudo::Method),
            ":scheme" => Ok(HeaderOrPseudo::Scheme),
            ":authority" => Ok(HeaderOrPseudo::Authority),
            ":path" => Ok(HeaderOrPseudo::Path),
            ":status" => Ok(HeaderOrPseudo::Status),
            _ => HeaderName::try_from(value).map(HeaderOrPseudo::Header),
        }
    }
}

impl std::fmt::Display for HeaderOrPseudo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderOrPseudo::Header(h) => write!(f, "{}", h.as_str()),
            HeaderOrPseudo::Method => write!(f, ":method"),
            HeaderOrPseudo::Scheme => write!(f, ":scheme"),
            HeaderOrPseudo::Authority => write!(f, ":authority"),
            HeaderOrPseudo::Path => write!(f, ":path"),
            HeaderOrPseudo::Status => write!(f, ":status"),
        }
    }
}

impl Serialize for HeaderOrPseudo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            HeaderOrPseudo::Header(h) => h.as_str().serialize(serializer),
            HeaderOrPseudo::Method => ":method".serialize(serializer),
            HeaderOrPseudo::Scheme => ":scheme".serialize(serializer),
            HeaderOrPseudo::Authority => ":authority".serialize(serializer),
            HeaderOrPseudo::Path => ":path".serialize(serializer),
            HeaderOrPseudo::Status => ":status".serialize(serializer),
        }
    }
}

#[derive(Debug)]
pub enum RequestOrResponse<'a> {
    Request(&'a mut http::Request<PolyBody>),
    Response(&'a mut http::Response<PolyBody>),
}

pub fn apply_header_or_pseudo(rr: &mut RequestOrResponse, pseudo: &HeaderOrPseudo, raw: &[u8]) -> bool {
    match (rr, pseudo) {
        (RequestOrResponse::Request(req), HeaderOrPseudo::Method) => {
            if let Ok(m) = ::http::Method::from_bytes(raw) {
                *req.method_mut() = m;
                return true;
            }
        },
        (RequestOrResponse::Request(req), HeaderOrPseudo::Scheme) => {
            if let Ok(s) = ::http::uri::Scheme::try_from(raw) {
                let _ = modify_req_uri(req, |uri| {
                    uri.scheme = Some(s);
                    Ok(())
                });
                return true;
            }
        },
        (RequestOrResponse::Request(req), HeaderOrPseudo::Authority) => {
            if let Ok(a) = ::http::uri::Authority::try_from(raw) {
                let _ = modify_req_uri(req, |uri| {
                    uri.authority = Some(a);
                    if uri.scheme.is_none() {
                        // When authority is set, scheme must also be set
                        // TODO: do the same for HeaderOrPseudo::Scheme
                        uri.scheme = Some(Scheme::HTTP);
                    }
                    Ok(())
                });
                return true;
            }
        },
        (RequestOrResponse::Request(req), HeaderOrPseudo::Path) => {
            if let Ok(pq) = ::http::uri::PathAndQuery::try_from(raw) {
                let _ = modify_req_uri(req, |uri| {
                    uri.path_and_query = Some(pq);
                    Ok(())
                });
                return true;
            }
        },
        (RequestOrResponse::Response(resp), HeaderOrPseudo::Status) => {
            if let Some(code) = std::str::from_utf8(raw)
                .ok()
                .and_then(|s| s.parse::<u16>().ok())
                .and_then(|c| ::http::StatusCode::from_u16(c).ok())
            {
                *resp.status_mut() = code;
                return true;
            }
        },
        (RequestOrResponse::Response(resp), HeaderOrPseudo::Header(hn)) => {
            let Ok(hv) = HeaderValue::try_from(raw) else {
                return false;
            };
            resp.headers_mut().insert(hn, hv);
            return true;
        },
        (RequestOrResponse::Request(req), HeaderOrPseudo::Header(hn)) => {
            let Ok(hv) = HeaderValue::try_from(raw) else {
                return false;
            };
            req.headers_mut().insert(hn, hv);
            return true;
        },
        // Not applicable combination
        _ => {},
    }
    false
}

pub fn modify_req_uri(
    req: &mut http::Request<PolyBody>,
    f: impl FnOnce(&mut uri::Parts) -> crate::Result<()>,
) -> crate::Result<()> {
    let nreq = std::mem::take(req);
    let (mut head, body) = nreq.into_parts();
    let mut parts = head.uri.into_parts();
    f(&mut parts)?;
    head.uri = Uri::from_parts(parts)?;
    *req = Request::from_parts(head, body);
    Ok(())
}

pub fn get_request_pseudo_headers(req: &http::Request<PolyBody>) -> Vec<(HeaderOrPseudo, String)> {
    let mut out = Vec::with_capacity(4);
    if let Some(v) = get_pseudo_header_value(&HeaderOrPseudo::Method, req) {
        out.push((HeaderOrPseudo::Method, v));
    }
    if let Some(v) = get_pseudo_header_value(&HeaderOrPseudo::Scheme, req) {
        out.push((HeaderOrPseudo::Scheme, v));
    }
    if let Some(v) = get_pseudo_header_value(&HeaderOrPseudo::Authority, req) {
        out.push((HeaderOrPseudo::Authority, v));
    }
    if let Some(v) = get_pseudo_header_value(&HeaderOrPseudo::Path, req) {
        out.push((HeaderOrPseudo::Path, v));
    }
    out
}

pub fn get_pseudo_header_value(pseudo: &HeaderOrPseudo, req: &http::Request<PolyBody>) -> Option<String> {
    match pseudo {
        HeaderOrPseudo::Method => Some(req.method().to_string()),
        HeaderOrPseudo::Scheme => req.uri().scheme().map(|s| s.to_string()),
        HeaderOrPseudo::Authority => req
            .uri()
            .authority()
            .map(|a| a.to_string())
            .or_else(|| req.headers().get("host").and_then(|h| h.to_str().ok().map(|s| s.to_string()))),
        HeaderOrPseudo::Path => {
            req.uri().path_and_query().map(|pq| pq.to_string()).or_else(|| Some(req.uri().path().to_string()))
        },
        HeaderOrPseudo::Status => None,    // no status for requests
        HeaderOrPseudo::Header(_) => None, // skip regular headers
    }
}

pub async fn read_body_with_limit(body: PolyBody, limit: usize) -> Result<Bytes, orion_error::Error> {
    Limited::new(body, limit).collect().await.map(|col| col.to_bytes()).map_err(|e| orion_error::Error::from(e))
}
