//! The "framework": route patterns with `:params`, handlers as trait
//! objects, and a middleware chain.
//!
//! ```text
//! router.handle(req) ─► Logger ─► RateLimiter ─► dispatch ─► handler
//!                       each middleware gets `next` and decides
//!                       whether (and when) to call `next.run(req)`
//! ```

use std::collections::HashMap;

use crate::http::{Request, Response};

/// Anything that turns a request into a response. The blanket impl below
/// makes every matching closure or `fn` a `Handler`, so you can write
/// `router.get("/x", |req: &Request| …)`. `Send + Sync` is required because
/// all worker threads share one router.
pub trait Handler: Send + Sync + 'static {
    fn call(&self, req: &Request) -> Response;
}

impl<F> Handler for F
where
    F: Fn(&Request) -> Response + Send + Sync + 'static,
{
    fn call(&self, req: &Request) -> Response {
        self(req)
    }
}

/// Code that wraps every request. Call `next.run(req)` to continue down the
/// chain, and look at the response it returns. Or return early without
/// calling it: that's how the rate limiter answers 429.
pub trait Middleware: Send + Sync + 'static {
    fn handle(&self, req: Request, next: Next<'_>) -> Response;
}

/// The rest of the chain: the middleware not yet run, then the routes.
/// `run` takes `self` by value, so each `Next` can be used only once.
pub struct Next<'a> {
    rest: &'a [Box<dyn Middleware>],
    router: &'a Router,
}

impl Next<'_> {
    pub fn run(self, req: Request) -> Response {
        match self.rest.split_first() {
            Some((first, rest)) => first.handle(
                req,
                Next {
                    rest,
                    router: self.router,
                },
            ),
            None => self.router.dispatch(req),
        }
    }
}

#[derive(Debug, PartialEq)]
enum Segment {
    Literal(String),
    Param(String),
}

struct Route {
    method: &'static str,
    pattern: Vec<Segment>,
    handler: Box<dyn Handler>,
}

#[derive(Default)]
pub struct Router {
    routes: Vec<Route>,
    middleware: Vec<Box<dyn Middleware>>,
}

impl Router {
    pub fn new() -> Router {
        Router::default()
    }

    pub fn get(self, pattern: &str, handler: impl Handler) -> Router {
        self.route("GET", pattern, handler)
    }

    pub fn post(self, pattern: &str, handler: impl Handler) -> Router {
        self.route("POST", pattern, handler)
    }

    pub fn patch(self, pattern: &str, handler: impl Handler) -> Router {
        self.route("PATCH", pattern, handler)
    }

    pub fn delete(self, pattern: &str, handler: impl Handler) -> Router {
        self.route("DELETE", pattern, handler)
    }

    /// `impl Handler` is generic (one copy per closure type), but storing
    /// different closures in one `Vec` needs a single type: `Box<dyn Handler>`.
    pub fn route(mut self, method: &'static str, pattern: &str, handler: impl Handler) -> Router {
        let pattern = segments(pattern)
            .map(|s| match s.strip_prefix(':') {
                Some(name) => Segment::Param(name.to_string()),
                None => Segment::Literal(s.to_string()),
            })
            .collect();
        self.routes.push(Route {
            method,
            pattern,
            handler: Box::new(handler),
        });
        self
    }

    /// Adds middleware. The first one added is the outermost.
    pub fn wrap(mut self, middleware: impl Middleware) -> Router {
        self.middleware.push(Box::new(middleware));
        self
    }

    /// The entry point: runs the middleware chain, then the matching route.
    pub fn handle(&self, req: Request) -> Response {
        Next {
            rest: &self.middleware,
            router: self,
        }
        .run(req)
    }

    /// 404 if no pattern matches the path. 405 (with `Allow`) if some do,
    /// but not for this method.
    fn dispatch(&self, mut req: Request) -> Response {
        let mut allowed = Vec::new();
        for route in &self.routes {
            let Some(params) = match_path(&route.pattern, &req.path) else {
                continue;
            };
            if route.method == req.method {
                req.params = params;
                return route.handler.call(&req);
            }
            allowed.push(route.method);
        }
        if allowed.is_empty() {
            Response::error(404, "not found")
        } else {
            Response::error(405, "method not allowed").with_header("Allow", &allowed.join(", "))
        }
    }
}

/// Empty segments are skipped, so `/todos/` and `//todos` match `/todos`.
fn segments(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|s| !s.is_empty())
}

fn match_path(pattern: &[Segment], path: &str) -> Option<HashMap<String, String>> {
    let parts: Vec<&str> = segments(path).collect();
    if parts.len() != pattern.len() {
        return None;
    }
    let mut params = HashMap::new();
    for (segment, part) in pattern.iter().zip(parts) {
        match segment {
            Segment::Literal(lit) if lit == part => {}
            Segment::Literal(_) => return None,
            Segment::Param(name) => {
                params.insert(name.clone(), part.to_string());
            }
        }
    }
    Some(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// Each handler reports its name and the params it saw.
    fn named(name: &'static str) -> impl Handler {
        move |req: &Request| Response::json(200, &json!({ "route": name, "params": req.params }))
    }

    fn router() -> Router {
        Router::new()
            .get("/todos", named("list"))
            .post("/todos", named("create"))
            .get("/todos/:id", named("show"))
            .delete("/todos/:id", named("delete"))
            .get("/users/:user/todos/:id", named("nested"))
    }

    fn body(resp: &Response) -> Value {
        serde_json::from_slice(&resp.body).unwrap()
    }

    #[test]
    fn matches_routes() {
        let router = router();
        for (method, path, route, params) in [
            ("GET", "/todos", "list", json!({})),
            ("GET", "/todos/", "list", json!({})),
            ("POST", "/todos", "create", json!({})),
            ("GET", "/todos/42", "show", json!({ "id": "42" })),
            ("DELETE", "/todos/abc", "delete", json!({ "id": "abc" })),
            (
                "GET",
                "/users/ann/todos/7",
                "nested",
                json!({ "user": "ann", "id": "7" }),
            ),
        ] {
            let resp = router.handle(Request::new(method, path));
            assert_eq!(
                body(&resp),
                json!({ "route": route, "params": params }),
                "{method} {path}"
            );
        }
    }

    #[test]
    fn unknown_paths_are_404_and_wrong_methods_405() {
        let router = router();
        for path in ["/", "/nope", "/todos/1/extra", "/users/ann"] {
            let resp = router.handle(Request::new("GET", path));
            assert_eq!(
                (resp.status, body(&resp)),
                (404, json!({ "error": "not found" })),
                "{path}"
            );
        }
        let resp = router.handle(Request::new("PUT", "/todos/1"));
        assert_eq!(
            (resp.status, resp.header("Allow")),
            (405, Some("GET, DELETE"))
        );
    }

    struct Tag(&'static str);

    impl Middleware for Tag {
        fn handle(&self, req: Request, next: Next<'_>) -> Response {
            if req.path == "/blocked" {
                return Response::error(403, self.0); // short-circuit
            }
            next.run(req).with_header("X-Seen-By", self.0)
        }
    }

    #[test]
    fn middleware_runs_outermost_first_and_can_short_circuit() {
        let router = router().wrap(Tag("outer")).wrap(Tag("inner"));
        let resp = router.handle(Request::new("GET", "/todos"));
        let seen: Vec<&str> = resp
            .headers
            .iter()
            .filter(|(n, _)| n == "X-Seen-By")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(seen, ["inner", "outer"]); // inner finishes first, so it tags first
        assert_eq!(
            body(&router.handle(Request::new("GET", "/blocked"))),
            json!({ "error": "outer" })
        );
    }
}
