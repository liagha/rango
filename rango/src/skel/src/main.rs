use std::process::ExitCode;

use {CRATE}::Message;
use rango::Rango;
use rango::authentication::Current;
use rango::prelude::*;

#[derive(Template)]
#[template(path = "index.html", askama = rango::askama)]
struct Index {
    messages: Vec<Message>,
    user: String,
}

async fn index(
    repository: Repository<Message>,
    current: Current,
) -> Result<Response, Error> {
    let messages = repository.all().await.map_err(Error::from)?;
    render(Index {
        messages,
        user: current.0.map(|user| user.username).unwrap_or_default(),
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    Rango::serve(env!("CARGO_MANIFEST_DIR"))
        .model::<Message>()
        .routes(Routes::new().route("/", get(index)))
        .authentication(|authentication| authentication.signup(true))
        .run()
        .await
}