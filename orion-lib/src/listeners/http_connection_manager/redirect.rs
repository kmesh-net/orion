// Copyright 2025 The kmesh Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

use super::{RequestHandler, TransactionHandler};

use crate::{
    body::{body_with_metrics::BodyWithMetrics, body_with_timeout::BodyWithTimeout},
    Error, PolyBody, Result,
};
use http::{
    header::LOCATION,
    uri::{Authority, Parts as UriParts, PathAndQuery, Scheme},
    HeaderValue, StatusCode, Uri,
};
use hyper::{body::Incoming, Request, Response};
use orion_configuration::config::network_filters::http_connection_manager::route::{
    AuthorityRedirect, RedirectAction, RouteMatchResult,
};
use orion_error::Context;
use std::str::FromStr;

impl RequestHandler<(Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>, RouteMatchResult)> for &RedirectAction {
    async fn to_response(
        self,
        _trans_handler: &TransactionHandler,
        (request, route_match_result): (Request<BodyWithMetrics<BodyWithTimeout<Incoming>>>, RouteMatchResult),
    ) -> Result<Response<PolyBody>> {
        let (parts, _) = request.into_parts();
        let mut rsp = Response::builder().status(StatusCode::from(self.response_code)).version(parts.version);

        let UriParts { scheme: orig_scheme, authority: orig_authority, path_and_query: orig_path_and_query, .. } =
            parts.uri.into_parts();
        let orig_host = orig_authority.as_ref().map(Authority::host);
        let orig_port = orig_authority.as_ref().and_then(Authority::port_u16);
        let authority = match (self.authority_redirect.as_ref(), (orig_host, orig_port)) {
            //no redirect
            (None, _) => orig_authority,
            //full authority redirect OR host redirect with no port in the original uri
            (Some(AuthorityRedirect::AuthorityRedirect(a)), _)
            | (Some(AuthorityRedirect::HostRedirect(a)), (_, None)) => Some(a.clone()),
            (Some(AuthorityRedirect::HostRedirect(h)), (_, Some(port))) => {
                if (orig_scheme == Some(Scheme::HTTP) && port == 80)
                    || (orig_scheme == Some(Scheme::HTTPS) && port == 443)
                {
                    //strip port
                    Some(h.clone())
                } else {
                    let uri = format!("{h}:{port}");
                    Some(Authority::from_str(&uri).with_context_msg("invalid uri \"{uri}\"")?)
                }
            },
            // port redirect with a host in the original uri
            (Some(AuthorityRedirect::PortRedirect(port)), (Some(h), _)) => {
                let uri = format!("{h}:{port}");
                Some(Authority::from_str(&uri).with_context_msg("invalid uri \"{uri}\"")?)
            },
            // a port redirection with no known host
            (Some(AuthorityRedirect::PortRedirect(_)), (None, _)) => {
                return Err("tried to perform a port redirection with no host given".into());
            },
        };

        // strip query if specified
        let orig_path_and_query = if let Some(orig) = orig_path_and_query {
            if orig.query().is_some() && self.strip_query {
                Some(PathAndQuery::from_str(orig.path()).with_context_msg("failed to strip query")?)
            } else {
                Some(orig)
            }
        } else {
            None
        };

        let scheme = self.scheme_rewrite_specifier.clone().or(orig_scheme);

        // if this replacement yields a query, it will always overwrite the existing query
        let path_and_query = if let Some(prs) = self.path_rewrite_specifier.as_ref() {
            if let Some(replacement) = prs
                .apply(orig_path_and_query.as_ref(), &route_match_result)
                .with_context_msg("invalid path or query following replacement")?
            {
                Some(replacement)
            } else {
                orig_path_and_query
            }
        } else {
            orig_path_and_query
        };

        let new_uri = Uri::from_parts({
            let mut parts = UriParts::default();
            parts.authority = authority;
            parts.scheme = scheme;
            parts.path_and_query = path_and_query;
            parts
        })
        .with_context_msg("failed to reconstruct uri after applying redirect params")?;
        let redirect_target =
            HeaderValue::from_str(&new_uri.to_string()).with_context_msg("couldn't convert uri to header value")?;
        rsp.headers_mut().and_then(|hm| hm.insert(LOCATION, redirect_target));
        rsp.body(PolyBody::default()).map_err(Error::from)
    }
}
