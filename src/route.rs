use crate::models::{block, pool};
use crate::{email, qury, sql, AppConfig};
use rocket::http::{Cookie, CookieJar, Header, Status};
use rocket::response::status;
use rocket::response::Responder;
use rocket::serde::json::Json;
use rocket::{Request, State};
use rocket_db_pools::Connection;

#[derive(Debug, Clone, PartialEq)]
pub struct Cors<R>(pub R);

impl<'r, 'o: 'r, R: Responder<'r, 'o>> Responder<'r, 'o> for Cors<R> {
    #[inline]
    fn respond_to(self, req: &'r Request<'_>) -> rocket::response::Result<'o> {
        rocket::Response::build_from(self.0.respond_to(req)?)
            .header(Header::new("Access-Control-Allow-Origin", "*"))
            .ok()
    }
}

#[get("/pools/top?<since>")]
pub(crate) async fn pools_top(
    mut db: Connection<sql::AuthDb>,
    since: Option<&str>,
) -> Cors<Json<Vec<pool::Pool>>> {
    let latest_block = block::find_latest(&mut db).await.unwrap();
    let hours_ago = match since {
        Some(since) => since_parse(since),
        None => 24,
    };
    log::info!("/pools/top?since={}", since.unwrap_or("<none>"));
    Cors(Json(
        sql::top_pools(
            db,
            &latest_block.number.hours_ago(hours_ago),
            &latest_block.number,
        )
        .await,
    ))
}

#[get("/pools/for?<token_contract_address>")]
pub(crate) async fn pools_for(
    db: Connection<sql::AuthDb>,
    token_contract_address: &str,
) -> Cors<Json<Vec<pool::Pool>>> {
    Cors(Json(sql::pools_for(db, token_contract_address).await))
}

fn since_parse(since: &str) -> u32 {
    u32::from_str_radix(since, 10).unwrap()
}

#[get("/pools/<pool_id>/since?<price0>&<price1>")]
pub(crate) async fn pools_since(
    db: Connection<sql::AuthDb>,
    pool_id: &str,
    price0: Option<f64>,
    price1: Option<f64>,
) -> Result<Cors<Json<qury::PoolSinceResponse>>, Json<String>> {
    match qury::pool_price_since(db, pool_id, price0, price1).await {
        Ok(zo) => Ok(Cors(Json(zo))),
        Err(e) => Err(Json(e)),
    }
}

#[get("/auth/<token>")]
pub(crate) async fn auth(
    db: Connection<sql::AuthDb>,
    cookies: &CookieJar<'_>,
    token: &str,
) -> Cors<status::Custom<Json<String>>> {
    Cors(match sql::find_by_token(db, token).await {
        Some(account) => {
            let token_cookie = Cookie::build(("token", token.to_owned()));
            cookies.add(token_cookie);
            status::Custom(Status::Ok, Json(account.email))
        }
        None => status::Custom(Status::Unauthorized, Json("bad token".to_owned())),
    })
}

#[post("/register/<email>")]
pub(crate) async fn register(
    app_config: &State<AppConfig>,
    db: Connection<sql::AuthDb>,
    email: &str,
) -> Cors<Json<String>> {
    let acct = sql::find_or_create_by_email(db, email).await;
    let url = format!("{}{}", app_config.site, acct.token);
    let email = email::build_message(&app_config.from_name, &app_config.from_email, &acct, &url);
    email::send_email(&app_config.smtp, email).await;
    Cors(Json(format!("{}", acct.email)))
}

#[cfg(test)]
mod test {
    use crate::rocket;
    use rocket::http::Status;
    use rocket::local::blocking::Client;
    use rocket::serde::json;

    #[test]
    fn register() {
        let email = "a@b.c";
        let client = Client::tracked(rocket()).expect("valid rocket instance");
        let response = client.post(format!("/register/{}", email)).dispatch();
        assert_eq!(response.status(), Status::Ok);
        let body_json = json::from_str::<String>(&response.into_string().unwrap()).unwrap();
        assert_eq!(body_json, email);
    }

    #[test]
    fn auth() {
        let client = Client::tracked(rocket()).expect("valid rocket instance");
        let token = "non-existant-token";
        let response = client.get(format!("/auth/{}", token)).dispatch();
        assert_eq!(response.status(), Status::new(401));
        assert_eq!(response.into_string().unwrap(), "\"bad token\"");
    }
}
