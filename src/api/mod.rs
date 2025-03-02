pub mod health;
pub mod call;

use rocket::{Route, routes};

/// Get all routes for the API module
pub fn routes() -> Vec<Route> {
    routes![
        health::health,
        call::make_call,
    ]
}
