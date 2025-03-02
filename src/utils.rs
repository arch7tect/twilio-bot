use rocket::http::ContentType;
use rocket::request::Request;
use rocket::response::{self, Responder, Response};
use std::io::Cursor;

/// XML response type for Rocket handlers
#[derive(Debug)]
pub struct Xml<T>(pub T);

impl<'r, T: Into<String>> Responder<'r, 'static> for Xml<T> {
    fn respond_to(self, _: &'r Request<'_>) -> response::Result<'static> {
        let string = self.0.into();
        Response::build()
            .sized_body(string.len(), Cursor::new(string))
            .header(ContentType::new("application", "xml"))
            .ok()
    }
}
