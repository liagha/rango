use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use askama::Template;
use axum::{
    extract::{Extension, Form, OriginalUri, Path},
    http::{HeaderMap, Uri, header::COOKIE},
    routing::{get, post},
};
use rango::{
    Error, Repo, Response, Row, Store, Value,
    csrf::Token,
    model::{Field, Kind, Model},
    urls::Routes,
    view::{self, render},
};

#[derive(Clone)]
struct Registered {
    table: &'static str,
}

pub struct Admin {
    routes: Routes,
    models: Arc<Mutex<Vec<Registered>>>,
}

impl Admin {
    pub fn new() -> Self {
        let models = Arc::new(Mutex::new(Vec::new()));
        let guard = models.clone();
        let routes = Routes::new().route(
            "/",
            get(move |store: Extension<Arc<dyn Store>>, OriginalUri(uri): OriginalUri| {
                let models = guard.clone();
                async move { dashboard(models, store, uri).await }
            }),
        );
        Self { routes, models }
    }

    pub fn model<M: Model>(mut self) -> Self {
        if let Ok(mut models) = self.models.lock() {
            models.push(Registered { table: M::table() });
        }
        self.routes = self.routes.merge(model_routes::<M>(M::table()));
        self
    }

    pub fn routes(self) -> Routes {
        self.routes
    }
}

impl Default for Admin {
    fn default() -> Self {
        Self::new()
    }
}

fn model_routes<M: Model>(table: &str) -> Routes {
    Routes::new()
        .route(format!("/{table}/"), get(list::<M>))
        .route(
            format!("/{table}/new/"),
            get(show_new::<M>).post(create::<M>),
        )
        .route(format!("/{table}/{{id}}/"), get(detail::<M>))
        .route(
            format!("/{table}/{{id}}/edit/"),
            get(show_edit::<M>).post(replace::<M>),
        )
        .route(format!("/{table}/{{id}}/delete/"), post(remove::<M>))
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct Dashboard {
    entries: Vec<Entry>,
}

struct Entry {
    href: String,
    title: String,
    count: String,
}

#[derive(Template)]
#[template(path = "list.html")]
struct List {
    title: &'static str,
    columns: Vec<String>,
    rows: Vec<Item>,
}

struct Item {
    id: String,
    cells: Vec<String>,
}

#[derive(Template)]
#[template(path = "form.html")]
struct FormView {
    title: String,
    token: String,
    inputs: Vec<String>,
}

#[derive(Template)]
#[template(path = "detail.html")]
struct Detail {
    title: &'static str,
    id: String,
    pairs: Vec<Pair>,
    token: String,
}

struct Pair {
    name: String,
    value: String,
}

fn token(headers: &HeaderMap, guard: Option<Extension<Token>>) -> String {
    match guard {
        Some(Extension(token)) => token.0.clone(),
        None => cookie(headers),
    }
}

fn cookie(headers: &HeaderMap) -> String {
    headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.split(';').map(str::trim).find_map(|part| {
                let (name, value) = part.split_once('=')?;
                (name == "csrf").then(|| value.to_string())
            })
        })
        .unwrap_or_default()
}

fn back(uri: &Uri, drop: usize) -> String {
    let mut parts: Vec<&str> = uri.path().trim_matches('/').split('/').collect();
    for _ in 0..drop {
        parts.pop();
    }
    format!("/{}/", parts.join("/"))
}

fn cell(values: &[Value], i: usize) -> String {
    match values.get(i) {
        Some(Value::Str(value)) => value.clone(),
        Some(Value::Int(value)) => value.to_string(),
        Some(Value::Float(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Null) | None => String::new(),
    }
}

fn id_of(values: &[Value], fields: &[Field]) -> String {
    fields
        .iter()
        .position(|field| field.kind == Kind::Id)
        .and_then(|i| values.get(i))
        .map(|value| match value {
            Value::Int(id) => id.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

fn positions(fields: &[Field]) -> Vec<usize> {
    fields
        .iter()
        .enumerate()
        .filter(|(_, field)| field.kind != Kind::Id)
        .map(|(i, _)| i)
        .collect()
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn display(value: Option<&Value>) -> String {
    match value {
        Some(Value::Str(text)) => text.clone(),
        Some(Value::Int(number)) => number.to_string(),
        Some(Value::Float(number)) => number.to_string(),
        Some(Value::Bool(true)) => "on".into(),
        Some(Value::Bool(false)) | Some(Value::Null) | None => String::new(),
    }
}

fn input(field: &Field, value: Option<&Value>) -> String {
    let name = field.name;
    let label = format!(r#"<label for="admin-{name}">{name}</label>"#);
    let kind = match &field.kind {
        Kind::Optional(inner) => inner.as_ref(),
        kind => kind,
    };
    match kind {
        Kind::Id | Kind::Optional(_) => String::new(),
        Kind::Str => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="text" value="{}">"#,
                escape(&display(value))
            )
        }
        Kind::Int | Kind::DateTime | Kind::Float => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="number" value="{}">"#,
                escape(&display(value))
            )
        }
        Kind::Bool => {
            let checked = matches!(value, Some(Value::Bool(true)));
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="checkbox"{}>"#,
                if checked { " checked" } else { "" }
            )
        }
    }
}

fn bad(field: &Field, want: &str) -> Error {
    Error::BadRequest(format!("{} must be {want}", field.name))
}

fn parse(field: &Field, raw: Option<&String>) -> Result<Value, Error> {
    let kind = match &field.kind {
        Kind::Optional(inner) => inner.as_ref(),
        kind => kind,
    };
    let raw = raw.map(String::as_str).unwrap_or("");
    if raw.is_empty() && matches!(kind, Kind::Bool) {
        return Ok(Value::bool(false));
    }
    if raw.is_empty() {
        return if field.kind.is_optional() {
            Ok(Value::Null)
        } else {
            Err(Error::BadRequest(format!("{} is required", field.name)))
        };
    }
    match kind {
        Kind::Id | Kind::Optional(_) => Ok(Value::Null),
        Kind::Str => Ok(Value::str(raw)),
        Kind::Int | Kind::DateTime => raw
            .parse::<i64>()
            .map(Value::int)
            .map_err(|_| bad(field, "an integer")),
        Kind::Float => raw
            .parse::<f64>()
            .map(Value::float)
            .map_err(|_| bad(field, "a number")),
        Kind::Bool => Ok(Value::bool(raw == "on")),
    }
}

fn entries(base: &str, models: &[Registered]) -> Vec<(String, String)> {
    models
        .iter()
        .map(|model| (format!("{base}/{}/", model.table), model.table.to_string()))
        .collect()
}

async fn dashboard(
    models: Arc<Mutex<Vec<Registered>>>,
    store: Extension<Arc<dyn Store>>,
    uri: Uri,
) -> Result<Response, Error> {
    let registered: Vec<Registered> = models
        .lock()
        .map(|models| models.clone())
        .map_err(|_| Error::Server("admin registry".into()))?;
    let base = uri.path().trim_end_matches('/');
    let mut items = Vec::new();
    for (href, title) in entries(base, &registered) {
        let rows = store
            .fetch(&format!("SELECT COUNT(*) FROM {title}"), &[])
            .await
            .unwrap_or_default();
        let count = match rows.first().and_then(|row| row.get(0)) {
            Some(Value::Int(number)) => number.to_string(),
            _ => "0".into(),
        };
        items.push(Entry { href, title, count });
    }
    render(Dashboard { entries: items })
}

async fn list<M: Model>(repo: Repo<M>) -> Result<Response, Error> {
    let fields = M::fields();
    let at = positions(&fields);
    let columns: Vec<String> = at.iter().map(|&i| fields[i].name.to_string()).collect();
    let mut rows = Vec::new();
    for model in repo.all().await? {
        let values = full(&model, &fields);
        rows.push(Item {
            id: id_of(&values, &fields),
            cells: at.iter().map(|&i| cell(&values, i)).collect(),
        });
    }
    render(List {
        title: M::table(),
        columns,
        rows,
    })
}

fn full<M: Model>(model: &M, fields: &[Field]) -> Vec<Value> {
    let mut out = vec![Value::int(model.id())];
    let mut values = model.row().into_iter();
    for field in fields {
        if field.kind == Kind::Id {
            continue;
        }
        out.push(values.next().unwrap_or(Value::Null));
    }
    out
}

async fn detail<M: Model>(
    repo: Repo<M>,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    let model = repo.get(id).await?.ok_or(Error::NotFound)?;
    let fields = M::fields();
    let values = full(&model, &fields);
    let pairs = fields
        .iter()
        .enumerate()
        .map(|(i, field)| Pair {
            name: field.name.to_string(),
            value: cell(&values, i),
        })
        .collect();
    render(Detail {
        title: M::table(),
        id: id.to_string(),
        pairs,
        token: token(&headers, guard),
    })
}

async fn show_new<M: Model>(
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
) -> Result<Response, Error> {
    let inputs = M::fields()
        .iter()
        .filter(|field| field.kind != Kind::Id)
        .map(|field| input(field, field.default.as_ref()))
        .collect();
    render(FormView {
        title: format!("New {}", M::table()),
        token: token(&headers, guard),
        inputs,
    })
}

async fn create<M: Model>(
    repo: Repo<M>,
    OriginalUri(uri): OriginalUri,
    Form(map): Form<HashMap<String, String>>,
) -> Result<Response, Error> {
    let fields = M::fields();
    let mut values = Vec::with_capacity(fields.len());
    for field in &fields {
        if field.kind == Kind::Id {
            values.push(Value::int(0));
            continue;
        }
        values.push(parse(field, map.get(field.name))?);
    }
    let mut model = M::from_row(&Row { values })?;
    repo.save(&mut model).await?;
    Ok(view::redirect(&back(&uri, 1)))
}

async fn show_edit<M: Model>(
    repo: Repo<M>,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    let model = repo.get(id).await?.ok_or(Error::NotFound)?;
    let values = model.row();
    let mut slots = values.iter();
    let mut inputs = Vec::new();
    for field in M::fields() {
        if field.kind == Kind::Id {
            continue;
        }
        inputs.push(input(&field, slots.next()));
    }
    render(FormView {
        title: format!("Edit {} {id}", M::table()),
        token: token(&headers, guard),
        inputs,
    })
}

async fn replace<M: Model>(
    repo: Repo<M>,
    OriginalUri(uri): OriginalUri,
    Path(id): Path<i64>,
    Form(map): Form<HashMap<String, String>>,
) -> Result<Response, Error> {
    repo.get(id).await?.ok_or(Error::NotFound)?;
    let fields = M::fields();
    let mut values = vec![Value::int(id)];
    for field in &fields {
        if field.kind == Kind::Id {
            continue;
        }
        values.push(parse(field, map.get(field.name))?);
    }
    let model = M::from_row(&Row { values })?;
    repo.update(&model).await?;
    Ok(view::redirect(&back(&uri, 1)))
}

async fn remove<M: Model>(repo: Repo<M>, OriginalUri(uri): OriginalUri, Path(id): Path<i64>) -> Result<Response, Error> {
    repo.delete(id).await?;
    Ok(view::redirect(&back(&uri, 2)))
}
