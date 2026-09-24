use regex::Regex;
use serde_json::{Map, Value as Json};

use super::{Request, Response, error_page};

pub type Handler = Box<dyn Fn(&mut Request) -> Response + Send + Sync>;
/// A `constraints:` lambda.
pub type Constraint = fn(&Request) -> bool;

struct Route {
    method: &'static str,
    pattern: Regex,
    names: Vec<String>,
    handler: Handler,
    constraints: Vec<Constraint>,
}

/// The routes in match order, as `config/routes.rb` declares them.
#[derive(Default)]
pub struct Router {
    routes: Vec<Route>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(self, pattern: &str, handler: Handler) -> Self { self.route("GET", pattern, handler) }
    pub fn post(self, pattern: &str, handler: Handler) -> Self { self.route("POST", pattern, handler) }
    pub fn patch(self, pattern: &str, handler: Handler) -> Self { self.route("PATCH", pattern, handler) }
    pub fn put(self, pattern: &str, handler: Handler) -> Self { self.route("PUT", pattern, handler) }
    pub fn delete(self, pattern: &str, handler: Handler) -> Self { self.route("DELETE", pattern, handler) }

    pub fn route(mut self, method: &'static str, pattern: &str, handler: Handler) -> Self {
        let (pattern, names) = compile(pattern);
        self.routes.push(Route { method, pattern, names, handler, constraints: Vec::new() });
        self
    }

    /// `constraints:` on the route declared just before.
    pub fn constraint(mut self, constraint: Constraint) -> Self {
        self.routes.last_mut().expect("a route before `constraint`").constraints.push(constraint);
        self
    }

    /// Runs the first route whose method, path and constraints all match.
    /// Like Rails, a failed constraint moves on to the next route, and a
    /// HEAD request with no HEAD route of its own runs the matching GET
    /// route (the server leaves out the body). No match is a 404 page.
    pub fn call(&self, req: &mut Request) -> Response {
        let method = req.method.clone();
        if let Some(response) = self.dispatch(req, &method) {
            return response;
        }
        if method == "HEAD" {
            if let Some(response) = self.dispatch(req, "GET") {
                return response;
            }
        }
        error_page(404)
    }

    fn dispatch(&self, req: &mut Request, method: &str) -> Option<Response> {
        for route in &self.routes {
            if route.method != method {
                continue;
            }
            let Some(captures) = route.pattern.captures(&req.path) else { continue };
            let mut path = Map::new();
            for name in &route.names {
                if let Some(m) = captures.name(name) {
                    path.insert(name.clone(), Json::String(m.as_str().to_string()));
                }
            }
            // As in Rails, the constraint sees this route's path params; a
            // route it rejects leaves the params as they were.
            let before = req.params.clone();
            req.params.merge_path(path);
            if !route.constraints.iter().all(|constraint| constraint(req)) {
                req.params = before;
                continue;
            }
            if req.malformed_body {
                return Some(error_page(400));
            }
            return Some((route.handler)(req));
        }
        None
    }
}

/// `/posts/:id(.:format)` becomes `^/posts/(?P<id>[^/.?]+)(?:\.(?P<format>[^/.?]+))?$`;
/// a dynamic segment matches what Rails' default requirement does.
fn compile(pattern: &str) -> (Regex, Vec<String>) {
    let (path, format) = match pattern.strip_suffix("(.:format)") {
        Some(path) => (path, true),
        None => (pattern, false),
    };
    let mut source = String::from("^");
    let mut names = Vec::new();
    for segment in path.split('/').skip(1) {
        source.push('/');
        match segment.strip_prefix(':') {
            Some(name) => {
                source.push_str(&format!("(?P<{name}>[^/.?]+)"));
                names.push(name.to_string());
            }
            None => source.push_str(&regex::escape(segment)),
        }
    }
    if format {
        source.push_str(r"(?:\.(?P<format>[^/.?]+))?");
        names.push("format".to_string());
    }
    source.push('$');
    (Regex::new(&source).expect("route pattern compiles"), names)
}
