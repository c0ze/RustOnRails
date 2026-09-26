use super::{Handler, Request, Response, error_page};
use crate::{Error, Result};

/// What a Rails controller class declares. Each request gets a fresh
/// `Default` instance, whose fields are the controller's instance variables.
pub trait Controller: Default + 'static {
    /// `wrap_parameters`: the wrapper key and the attribute names it takes,
    /// or None for every body key (a controller with no model).
    fn wrap_parameters() -> Option<(&'static str, Option<&'static [&'static str]>)> {
        None
    }

    /// The `before_action` chain for `action`. `Some(response)` halts it,
    /// like rendering in a filter.
    fn before(&mut self, _req: &mut Request, _action: &str) -> Result<Option<Response>> {
        Ok(None)
    }

    /// `rescue_from`. The default re-raises.
    fn rescue(&mut self, _req: &mut Request, error: Error) -> Result<Response> {
        Err(error)
    }
}

/// A controller action: `def show`.
pub type Action<C> = fn(&mut C, &mut Request) -> Result<Response>;

/// The route handler for `controller#name`.
pub fn action<C: Controller>(name: &'static str, run: Action<C>) -> Handler {
    Box::new(move |req: &mut Request| dispatch::<C>(name, run, req))
}

fn dispatch<C: Controller>(name: &'static str, run: Action<C>, req: &mut Request) -> Response {
    let mut controller = C::default();
    if let Some((key, include)) = C::wrap_parameters().filter(|_| req.is_json()) {
        let body = req.body.clone();
        req.params.wrap(&body, key, include);
    }
    let outcome = match controller.before(req, name) {
        Ok(Some(halted)) => Ok(halted),
        Ok(None) => run(&mut controller, req),
        Err(error) => Err(error),
    };
    let outcome = match outcome {
        Err(error) => controller.rescue(req, error),
        done => done,
    };
    outcome.unwrap_or_else(|error| {
        // Rails logs every exception it turns into an error page.
        eprintln!("{} {} failed: {error}", req.method, req.path);
        error_response(&error)
    })
}

/// What Rails does with an exception nobody rescued: the status from
/// `rescue_responses` and the exceptions app's JSON page.
pub fn error_response(error: &Error) -> Response {
    let status = match error {
        Error::RecordNotFound { .. } => 404,
        Error::RecordInvalid { .. } | Error::RecordNotSaved { .. } => 422,
        Error::ParameterMissing { .. } => 400,
        _ => 500,
    };
    error_page(status)
}
