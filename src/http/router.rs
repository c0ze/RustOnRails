use regex::Regex;
use serde_json::{Map, Value as Json};

use super::{CookieKey, CookieOptions, Request, Response, Session, SessionStore, error_page};

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
    session: Option<SessionStore>,
    /// `config.action_dispatch.default_headers`, on every response a
    /// controller gives.
    default_headers: Vec<(&'static str, &'static str)>,
    /// The app's `public/<status>.html` pages, for an HTML request's errors.
    public_pages: Vec<(u16, &'static str)>,
    /// `config.action_dispatch.cookies_same_site_protection`.
    same_site: Option<&'static str>,
    /// `config.force_ssl` behind `assume_ssl`: HSTS, and secure cookies.
    force_ssl: bool,
}

impl Router {
    /// A router with Rails 8.1's defaults: `samesite=lax` cookies.
    pub fn new() -> Self {
        Self { same_site: Some("lax"), ..Self::default() }
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
    /// Rails' default headers (`X-Frame-Options` and the rest).
    pub fn default_headers(mut self, headers: &[(&'static str, &'static str)]) -> Self {
        self.default_headers = headers.to_vec();
        self
    }

    /// `public/404.html` and its kind, which Rails' exceptions app sends an
    /// HTML request.
    pub fn public_page(mut self, status: u16, html: &'static str) -> Self {
        self.public_pages.push((status, html));
        self
    }

    /// The cookie store (`ActionDispatch::Session::CookieStore, key: name`).
    pub fn session_store(self, name: &'static str) -> Self {
        self.session_store_with(name, CookieOptions::session())
    }

    /// The cookie store with its `secure:`, `httponly:`, `same_site:` and
    /// `path:` options.
    pub fn session_store_with(mut self, name: &'static str, options: CookieOptions) -> Self {
        self.session = Some(SessionStore { name, key: None, options });
        self
    }

    /// `cookies_same_site_protection` for `cookies[:name] =` (Rails' `lax`
    /// unless the app says otherwise).
    pub fn cookies_same_site(mut self, same_site: Option<&'static str>) -> Self {
        self.same_site = same_site;
        self
    }

    /// `config.force_ssl` behind `config.assume_ssl`: every request is
    /// HTTPS, so Rails' SSL middleware adds HSTS and marks cookies secure.
    pub fn force_ssl(mut self) -> Self {
        self.force_ssl = true;
        self
    }

    /// Whether the app has a session store, whose cookie needs a secret.
    pub fn needs_secret(&self) -> bool {
        self.session.as_ref().is_some_and(|store| store.key.is_none())
    }

    /// The app's `secret_key_base`, which encrypts the session cookie.
    pub fn secret_key_base(mut self, secret: Option<&str>) -> Self {
        if let Some(store) = &mut self.session {
            store.key = secret.map(CookieKey::derive);
        }
        self
    }

    /// Routes the request, then writes the cookies it set and the session
    /// it loaded, as Rails' cookie and session middleware do.
    pub fn call(&self, req: &mut Request) -> Response {
        let cookie = self.session.as_ref().and_then(|store| req.cookies.get(store.name));
        req.session = Session::new(self.session.clone(), cookie);
        let mut response = self.route_request(req);
        if response.raised {
            return self.failure(req.wants_json_errors(), response.status);
        }
        let mut cookies = req.cookies.headers(self.same_site);
        match req.session.header() {
            Ok(session) => cookies.extend(session),
            Err(error) => {
                eprintln!("{} {} failed: {error}", req.method, req.path);
                return self.failure(req.wants_json_errors(), 500);
            }
        }
        response.cookies.extend(cookies);
        // A render whose format came from Accept varies by it.
        if response.content_type.is_some() && req.negotiated() {
            response.headers.push(("Vary", "Accept".into()));
        }
        response.headers.extend(self.default_headers.iter().map(|(name, value)| (*name, value.to_string())));
        self.secured(response)
    }

    /// Rails' SSL middleware, outermost: HSTS on everything, and `secure`
    /// on each cookie that lacks it.
    fn secured(&self, mut response: Response) -> Response {
        if self.force_ssl {
            response.headers.push(("Strict-Transport-Security", "max-age=63072000; includeSubDomains".into()));
            for cookie in &mut response.cookies {
                let secure = cookie.split(';').skip(1).any(|part| part.trim().eq_ignore_ascii_case("secure"));
                if !secure {
                    cookie.push_str("; secure");
                }
            }
        }
        response
    }

    /// What Rails' exceptions app sends for `status`: JSON to a JSON
    /// request; to anything else the app's `public/<status>.html`, or an
    /// empty HTML page when it has none.
    /// A failed request's response: Rails' exceptions app, inside its SSL
    /// middleware, with no cookies. The server uses it for what fails
    /// outside the app, a panic or no database.
    pub(crate) fn failure(&self, wants_json: bool, status: u16) -> Response {
        self.secured(self.shown(wants_json, status))
    }

    fn shown(&self, wants_json: bool, status: u16) -> Response {
        if wants_json {
            return error_page(status);
        }
        let page = self.public_pages.iter().find(|(code, _)| *code == status).map_or("", |(_, html)| html);
        Response { raised: true, ..Response::html(status, page.to_string()) }
    }

    fn route_request(&self, req: &mut Request) -> Response {
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
