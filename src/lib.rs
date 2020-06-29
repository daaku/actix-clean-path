//! `Middleware` to clean request's URI, and redirect if necessary.
//!
//! Performs following:
//!
//! - Merges multiple `/` into one.
//! - Resolves and eliminates `..` and `.` if any.
//! - Appends a trailing `/` if one is not present, and there is no file extension.
//!
//! It will respond with a permanent redirect if the path was cleaned.
//!
//! ```rust
//! use actix_web::{web, App, HttpResponse};
//!
//! # fn main() {
//! let app = App::new()
//!     .wrap(actix_clean_path::CleanPath)
//!     .route("/", web::get().to(|| HttpResponse::Ok()));
//! # }
//! ```

use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::http::{self, PathAndQuery, Uri};
use actix_web::{Error, HttpResponse};
use futures_util::future::{ok, Either, LocalBoxFuture, Ready};
use std::task::{Context, Poll};

/// `Middleware` to clean request's URI, and redirect if necessary.
/// See module documenation for more.
#[derive(Default, Clone, Copy)]
pub struct CleanPath;

impl<S, B> Transform<S> for CleanPath
where
    S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type InitError = ();
    type Transform = CleanPathNormalization<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ok(CleanPathNormalization { service })
    }
}

#[doc(hidden)]
pub struct CleanPathNormalization<S> {
    service: S,
}

impl<S, B> Service for CleanPathNormalization<S>
where
    S: Service<Request = ServiceRequest, Response = ServiceResponse<B>, Error = Error>,
    S::Future: 'static,
{
    type Request = ServiceRequest;
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Either<
        Ready<Result<Self::Response, Error>>,
        LocalBoxFuture<'static, Result<Self::Response, Error>>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(cx)
    }

    fn call(&mut self, req: ServiceRequest) -> Self::Future {
        let original_path = req.uri().path();
        let trailing_slash = original_path.ends_with('/');

        // non-allocating fast path
        if !original_path.contains("/.")
            && !original_path.contains("//")
            && (has_ext(original_path) ^ trailing_slash)
        {
            return Either::Right(Box::pin(self.service.call(req)));
        }

        let mut path = path_clean::clean(&original_path);
        if path != "/" {
            if trailing_slash || !has_ext(&path) {
                path.push('/');
            }
        }

        if path != original_path {
            let mut parts = req.uri().clone().into_parts();
            let pq = parts.path_and_query.as_ref().unwrap();
            let path = if let Some(q) = pq.query() {
                format!("{}?{}", path, q)
            } else {
                path
            };
            parts.path_and_query = Some(PathAndQuery::from_maybe_shared(path).unwrap());
            let uri = Uri::from_parts(parts).unwrap();

            Either::Left(ok(req.error_response(actix_web::Error::from(
                HttpResponse::PermanentRedirect()
                    .header(http::header::LOCATION, uri.to_string())
                    .finish(),
            ))))
        } else {
            Either::Right(Box::pin(self.service.call(req)))
        }
    }
}

fn has_ext(path: &str) -> bool {
    path.rfind('.')
        .map(|index| {
            let sub = &path[index + 1..];
            !sub.is_empty() && !sub.contains('/')
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::CleanPath;
    use actix_web::test::{call_service, init_service, TestRequest};
    use actix_web::{http, web, App, HttpResponse};

    #[actix_rt::test]
    async fn test_clean() {
        let mut app = init_service(
            App::new()
                .wrap(CleanPath)
                .service(web::resource("/*").to(|| HttpResponse::Ok())),
        )
        .await;

        let cases = vec![
            ("/.", "/"),
            ("/..", "/"),
            ("/..//..", "/"),
            ("/./", "/"),
            ("//", "/"),
            ("///", "/"),
            ("///?a=1", "/?a=1"),
            ("///?a=1&b=2", "/?a=1&b=2"),
            ("//?a=1", "/?a=1"),
            ("//a//b//", "/a/b/"),
            ("//a//b//.", "/a/b/"),
            ("//a//b//../", "/a/"),
            ("//a//b//./", "/a/b/"),
            ("//m.js", "/m.js"),
            ("/a//b", "/a/b/"),
            ("/a//b/", "/a/b/"),
            ("/a//b//", "/a/b/"),
            ("/a//m.js", "/a/m.js"),
            ("/m.", "/m./"),
        ];
        for (given, clean) in cases.iter() {
            let req = TestRequest::with_uri(given).to_request();
            let res = call_service(&mut app, req).await;
            assert!(res.status().is_redirection(), "for {}", given);
            assert_eq!(
                &res.headers()
                    .get(http::header::LOCATION)
                    .unwrap()
                    .to_str()
                    .unwrap(),
                clean,
                "for {}",
                given,
            );
        }
    }

    #[actix_rt::test]
    async fn test_pristine() {
        let mut app = init_service(
            App::new()
                .wrap(CleanPath)
                .service(web::resource("/*").to(|| HttpResponse::Ok())),
        )
        .await;

        let cases = vec!["/", "/a/", "/a/b/", "/m.js", "/m./"];
        for given in cases.iter() {
            let req = TestRequest::with_uri(given).to_request();
            let res = call_service(&mut app, req).await;
            assert!(res.status().is_success(), "for {}", given);
        }
    }
}
