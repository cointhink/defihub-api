use crate::account::Account;
use mail_send::{self, mail_builder::MessageBuilder, SmtpClientBuilder};
use rocket::http::Status;
use rocket::response::status;
use rocket::State;
use rocket::{fairing::AdHoc, serde::Deserialize};
use rocket_db_pools::{Connection, Database};

mod account;
mod sql;

#[macro_use]
extern crate rocket;

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
pub struct AppConfig {
    smtp: String,
    site: String,
    from_name: String,
    from_email: String,
}

#[get("/auth/<token>")]
async fn auth(db: Connection<sql::AuthDb>, token: &str) -> status::Custom<String> {
    match sql::find_by_token(db, token).await {
        Some(account) => status::Custom(Status::Ok, account.email),
        None => status::Custom(Status::new(401), "bad token".to_owned()),
    }
}

#[get("/register/<email>")]
async fn register(
    app_config: &State<AppConfig>,
    db: Connection<sql::AuthDb>,
    email: &str,
) -> String {
    let acct = sql::find_or_create_by_email(db, email).await;
    let body = format!("{}/{}", app_config.site, acct.token);
    let email = build_message(&app_config.from_name, &app_config.from_email, &acct, &body);
    send_email(&app_config.smtp, email).await;
    format!("{}", acct.email)
}

#[launch]
fn rocket() -> _ {
    rocket::build()
        .attach(sql::AuthDb::init())
        .attach(AdHoc::config::<AppConfig>())
        .mount("/", routes![auth, register])
}

fn build_message<'b>(
    from_name: &'b str,
    from_email: &'b str,
    account: &'b Account,
    body: &'b str,
) -> MessageBuilder<'b> {
    MessageBuilder::new()
        .from((from_name, from_email))
        .to(account.email.as_str())
        .subject("Cointhink api token")
        .text_body(body)
}

async fn send_email<'b>(smtp_host: &str, email: MessageBuilder<'b>) {
    println!("smtp {} to {:?}", smtp_host, email);
    SmtpClientBuilder::new(smtp_host, 25)
        .allow_invalid_certs()
        .implicit_tls(false)
        .connect()
        .await
        .unwrap()
        .send(email)
        .await
        .unwrap();
}

#[cfg(test)]
mod test {
    use super::rocket;
    use rocket::http::Status;
    use rocket::local::blocking::Client;

    #[test]
    fn register() {
        let email = "a@b.c";
        let client = Client::tracked(rocket()).expect("valid rocket instance");
        let response = client.get(format!("/register/{}", email)).dispatch();
        assert_eq!(response.status(), Status::Ok);
        let body = response.into_string().unwrap();
        assert_eq!(body, email.to_string());
    }

    #[test]
    fn auth() {
        let client = Client::tracked(rocket()).expect("valid rocket instance");
        let token = "non-existant-token";
        let response = client.get(format!("/auth/{}", token)).dispatch();
        assert_eq!(response.status(), Status::new(401));
        assert_eq!(response.into_string().unwrap(), "bad token");
    }
}
