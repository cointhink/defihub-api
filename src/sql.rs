use rocket::fairing::AdHoc;
use rocket_db_pools::{
    sqlx::{self, database::HasArguments, query::Query, Postgres, Row},
    Connection, Database,
};

use crate::models::{
    account::{self, Account},
    block, coin,
    pool::{self, Pool},
    reserve,
};

#[derive(Database)]
#[database("auth_db")]
pub struct AuthDb(sqlx::PgPool);

// // on_ignite can still modify the rocket instance, so it returns build
// pub fn migrate() -> AdHoc {
//     AdHoc::on_ignite("SQLx query log", |build| async {
//         let db = AuthDb::fetch(&build).unwrap();
//         build
//     })
// }

// Log Statements https://docs.rs/rocket_db_pools/latest/src/rocket_db_pools/pool.rs.html#304
// if let Ok(level) = figment.extract_inner::<LogLevel>(rocket::Config::LOG_LEVEL) {
//     if !matches!(level, LogLevel::Normal | LogLevel::Off) {
//         opts = opts.log_statements(level.into())
//             .log_slow_statements(level.into(), Duration::default());
//     }
// }

pub fn migrate() -> AdHoc {
    AdHoc::on_liftoff("SQLx Migrate", |build| {
        Box::pin(async move {
            let db = AuthDb::fetch(&build).unwrap();
            match sqlx::migrate!("./sql").run(&**db).await {
                Ok(_) => (),
                Err(e) => error!("migration error: {}", e),
            }
        })
    })
}

pub fn query<DB>(sql: &str) -> Query<'_, DB, <DB as HasArguments<'_>>::Arguments>
where
    DB: rocket_db_pools::sqlx::Database,
{
    sqlx::query(sql)
}

impl Account {
    fn from_row(row: &<Postgres as rocket_db_pools::sqlx::Database>::Row) -> Account {
        Account {
            id: row.get::<String, &str>("id"),
            email: row.get::<String, &str>("email"),
            token: row.get::<String, &str>("token"),
        }
    }

    pub fn from_email(email: &str) -> Account {
        Account {
            id: account::get_nice_rand_str(),
            email: email.to_string(),
            token: account::get_nice_rand_str(),
        }
    }
}

pub async fn find_or_create_by_email(mut db: Connection<AuthDb>, email: &str) -> Account {
    match query("SELECT * FROM auth WHERE email = $1")
        .bind(email)
        .fetch_one(&mut **db)
        .await
    {
        Ok(row) => Account::from_row(&row),
        Err(_e) => {
            let account = Account::from_email(email);
            insert(db, &account).await;
            account
        }
    }
}

pub async fn find_by_token(mut db: Connection<AuthDb>, token: &str) -> Option<Account> {
    match query("SELECT * FROM auth WHERE token = $1")
        .bind(token)
        .fetch_one(&mut **db)
        .await
    {
        Ok(row) => Some(Account::from_row(&row)),
        Err(_e) => None,
    }
}

pub async fn insert(mut db: Connection<AuthDb>, account: &Account) {
    query("INSERT INTO auth values ($1, $2, $3)")
        .bind(account.id.as_str())
        .bind(account.email.as_str())
        .bind(account.token.as_str())
        .execute(&mut **db)
        .await
        .unwrap();
}

pub async fn pools_for(mut db: Connection<AuthDb>, token_contract_address: &str) -> Vec<Pool> {
    let sql = "select * from pools where token0 = $1 or token1 = $1 limit 20";
    match query(sql)
        .bind(token_contract_address)
        .fetch_all(&mut **db)
        .await
    {
        Ok(rows) => {
            let mut r = vec![];
            for pool_row in rows {
                r.push(Pool::from_row(&pool_row));
            }
            r
        }
        Err(_e) => vec![],
    }
}

pub async fn top_pairs_pools(
    mut db: Connection<AuthDb>,
    start_block: &block::Number,
    stop_block: &block::Number,
) -> Vec<(Pool, Pool, f64)> {
    let sql = " WITH latest_reserves AS
    (SELECT contract_address, block_number, x,y, ROW_NUMBER() OVER(PARTITION BY contract_address ORDER BY block_number)
      FROM reserves ORDER BY contract_address, block_number)
   SELECT p1.contract_address as p1_contract_address,
          p2.contract_address as p2_contract_address,
          least(lrp1.x, lrp2.x) AS qty_x,
          least(lrp1.y, lrp2.y) AS qty_y,
          (((lrp1.x::decimal/lrp1.y::decimal) - (lrp2.x::decimal/lrp2.y::decimal)) / (lrp1.x::decimal/lrp1.y::decimal)) as spread,
          least(lrp1.x::decimal * lrp1.y::decimal, lrp2.x::decimal * lrp2.y::decimal) *
             (((lrp1.x::decimal/lrp1.y::decimal) - (lrp2.x::decimal/lrp2.y::decimal)) / (lrp1.x::decimal/lrp1.y::decimal))::float as value
   FROM pools AS p1
   JOIN pools AS p2 ON p1.token0 = p2.token0 AND p1.token1 = p2.token1 AND p1.contract_address != p2.contract_address
   JOIN latest_reserves AS lrp1 ON p1.contract_address = lrp1.contract_address AND lrp1.row_number = 1
   JOIN latest_reserves AS lrp2 ON p2.contract_address = lrp2.contract_address AND lrp2.row_number = 1
   ORDER BY value ";
    match query(sql)
        .bind::<i32>(start_block.into())
        .bind::<i32>(stop_block.into())
        .fetch_all(&mut **db)
        .await
    {
        Ok(rows) => {
            let mut r = vec![];
            for row in rows {
                let pool1 = pool::find_by_address(&mut **db, row.get("p1_contract_address"))
                    .await
                    .unwrap();
                let pool2 = pool::find_by_address(&mut **db, row.get("p2_contract_address"))
                    .await
                    .unwrap();
                r.push((pool1, pool2, row.get("value")));
            }
            r
        }
        Err(_e) => vec![],
    }
}

pub async fn top_pools(
    mut db: Connection<AuthDb>,
    start_block: &block::Number,
    stop_block: &block::Number,
) -> Vec<Pool> {
    let sql = "select pool_contract_address, sum(in0) as sum_in0, sum(in0_eth) as sum_in0_eth, sum(in1) as sum_in1, sum(in1_eth) as sum_in1_eth, sum(in0_eth + in1_eth) as sum_eth, count(NULLIF(in0,0)) as count0, count(NULLIF(in1,0)) as count1 from swaps where block_number > $1 and block_number <= $2 group by pool_contract_address order by sum_eth desc limit 10";
    match query(sql)
        .bind::<i32>(start_block.into())
        .bind::<i32>(stop_block.into())
        .fetch_all(&mut **db)
        .await
    {
        Ok(rows) => {
            let mut r = vec![];
            for row in rows {
                let pool_contract_address = row.get("pool_contract_address");
                let mut pool = pool::find_by_address(&mut **db, pool_contract_address)
                    .await
                    .unwrap();
                let reserve = reserve::find_by_address(&mut **db, pool_contract_address)
                    .await
                    .unwrap();
                pool.reserve = Some(reserve);
                pool.reserve_summary = match pool.has_cash_token() {
                    true => Some(
                        reserve::summarize(
                            &mut **db,
                            pool_contract_address,
                            start_block,
                            stop_block,
                            pool.cash_token_is_1(),
                        )
                        .await,
                    ),
                    false => None,
                };
                pool.sum0 = Some(row.get::<sqlx::types::BigDecimal, &str>("sum_in0"));
                pool.sum0_eth = Some(row.get::<sqlx::types::BigDecimal, &str>("sum_in0_eth"));
                pool.sum1 = Some(row.get::<sqlx::types::BigDecimal, &str>("sum_in1"));
                pool.sum1_eth = Some(row.get::<sqlx::types::BigDecimal, &str>("sum_in1_eth"));
                pool.sum_eth = Some(row.get::<sqlx::types::BigDecimal, &str>("sum_eth"));
                pool.count0 = Some(row.get::<i64, &str>("count0"));
                pool.count1 = Some(row.get::<i64, &str>("count1"));
                let coin0 = coin::find_by_address(&mut **db, &pool.token0)
                    .await
                    .unwrap();
                pool.coin0 = Some(coin0);
                let coin1 = coin::find_by_address(&mut **db, &pool.token1)
                    .await
                    .unwrap();
                pool.coin1 = Some(coin1);
                r.push(pool)
            }
            r
        }
        Err(_e) => vec![],
    }
}
