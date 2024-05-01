pub(crate) fn to_actix_status(status: reqwest::StatusCode) -> actix_web::http::StatusCode {
    actix_web::http::StatusCode::from_u16(status.as_u16()).expect("status codes are always valid")
}
