use std::{
    cmp::Ordering,
    collections::HashMap,
    sync::{Arc, Mutex},
};

use askama::Template;
use axum::{
    extract::{Extension, Form, OriginalUri, Path, Query},
    http::{HeaderMap, Uri},
    routing::{get, post},
};
use rango::{
    Error, Repo, Response, Row, Store, Value,
    csrf::{Token, cookie},
    model::{Field, Kind, Model},
    urls::Routes,
    view::{self, render},
};

#[derive(Clone)]
struct Registered {
    table: &'static str,
}

pub trait AdminModel: Model {
    fn columns() -> Vec<&'static str> {
        Self::fields()
            .iter()
            .filter(|field| field.kind != Kind::Id)
            .map(|field| field.name)
            .collect()
    }

    fn search() -> Vec<&'static str> {
        Self::fields()
            .iter()
            .filter(|field| matches!(field.kind.flat(), Kind::Str))
            .map(|field| field.name)
            .collect()
    }

    fn readonly() -> Vec<&'static str> {
        Vec::new()
    }
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
            get(
                move |store: Extension<Arc<dyn Store>>, OriginalUri(uri): OriginalUri| {
                    let models = guard.clone();
                    async move { dashboard(models, store, uri).await }
                },
            ),
        );
        Self { routes, models }
    }

    pub fn model<M: AdminModel>(mut self) -> Self {
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

fn model_routes<M: AdminModel>(table: &str) -> Routes {
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
    q: String,
    sort: String,
    filters: Vec<String>,
    columns: Vec<Column>,
    rows: Vec<Item>,
    total: usize,
    page: usize,
    pages: usize,
    prev: Option<String>,
    next: Option<String>,
}

struct Column {
    name: String,
    marker: &'static str,
    href: String,
}

struct Item {
    id: String,
    cells: Vec<String>,
}

#[derive(Template)]
#[template(path = "form.html")]
struct FormView {
    title: String,
    model: &'static str,
    home: &'static str,
    up: &'static str,
    sub: String,
    token: String,
    inputs: Vec<String>,
    errors: Vec<String>,
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
        None => cookie(headers).unwrap_or_default(),
    }
}

fn back(uri: &Uri, drop: usize) -> String {
    let mut parts: Vec<&str> = uri.path().trim_matches('/').split('/').collect();
    for _ in 0..drop {
        parts.pop();
    }
    format!("/{}/", parts.join("/"))
}

const PAGE: usize = 25;

fn cell(values: &[Value], i: usize) -> String {
    text(values.get(i))
}

fn text(value: Option<&Value>) -> String {
    match value {
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

fn locate(names: &[&'static str], fields: &[Field]) -> Vec<usize> {
    let mut out = Vec::new();
    for name in names {
        if let Some(i) = fields
            .iter()
            .position(|field| field.name == *name && field.kind != Kind::Id)
            && !out.contains(&i)
        {
            out.push(i);
        }
    }
    out
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn input(field: &Field, value: Option<&Value>) -> String {
    control(
        field,
        &text(value),
        matches!(value, Some(Value::Bool(true))),
    )
}

fn input_raw(field: &Field, raw: &str) -> String {
    control(
        field,
        raw,
        matches!(field.kind.flat(), Kind::Bool) && raw == "on",
    )
}

fn locked(field: &Field, value: Option<&Value>) -> String {
    format!(
        r#"<label>{}</label><p>{}</p>"#,
        field.name,
        escape(&text(value))
    )
}

fn control(field: &Field, value: &str, checked: bool) -> String {
    let name = field.name;
    let label = format!(r#"<label for="admin-{name}">{name}</label>"#);
    match field.kind.flat() {
        Kind::Id | Kind::Optional(_) => String::new(),
        Kind::Str => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="text" value="{}">"#,
                escape(value)
            )
        }
        Kind::Int | Kind::DateTime | Kind::Float => {
            format!(
                r#"{label}<input id="admin-{name}" name="{name}" type="number" value="{}">"#,
                escape(value)
            )
        }
        Kind::Bool => {
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

fn value(field: &Field, raw: Option<&String>) -> Result<Value, Error> {
    let kind = field.kind.flat();
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

fn keep(
    fields: &[Field],
    values: &[Value],
    query: &str,
    params: &HashMap<String, String>,
    find: &[usize],
) -> bool {
    if !query.is_empty() {
        let found = find
            .iter()
            .any(|&i| text(values.get(i)).to_lowercase().contains(query));
        if !found {
            return false;
        }
    }
    fields.iter().enumerate().all(|(i, field)| {
        if field.kind == Kind::Id {
            return true;
        }
        match params.get(field.name) {
            Some(raw) if !raw.is_empty() => hit(field, values.get(i), raw),
            _ => true,
        }
    })
}

fn hit(field: &Field, value: Option<&Value>, raw: &str) -> bool {
    match field.kind.flat() {
        Kind::Id | Kind::Optional(_) => true,
        Kind::Str => text(value).to_lowercase().contains(&raw.to_lowercase()),
        Kind::Int | Kind::DateTime => match (value, raw.parse::<i64>()) {
            (Some(Value::Int(have)), Ok(want)) => *have == want,
            _ => false,
        },
        Kind::Float => match (value, raw.parse::<f64>()) {
            (Some(Value::Float(have)), Ok(want)) => *have == want,
            _ => false,
        },
        Kind::Bool => match value {
            Some(Value::Bool(have)) => *have == matches!(raw, "1" | "true" | "on" | "yes"),
            _ => false,
        },
    }
}

fn compare(one: Option<&Value>, other: Option<&Value>) -> Ordering {
    match (one, other) {
        (Some(Value::Int(one)), Some(Value::Int(other))) => one.cmp(other),
        (Some(Value::Float(one)), Some(Value::Float(other))) => one.total_cmp(other),
        (Some(Value::Str(one)), Some(Value::Str(other))) => one.cmp(other),
        (Some(Value::Bool(one)), Some(Value::Bool(other))) => one.cmp(other),
        (Some(Value::Null) | None, Some(Value::Null) | None) => Ordering::Equal,
        (Some(Value::Null) | None, _) => Ordering::Greater,
        (_, Some(Value::Null) | None) => Ordering::Less,
        _ => Ordering::Equal,
    }
}

fn sort_rows(fields: &[Field], rows: &mut [Vec<Value>], sort: &str) {
    if sort.is_empty() {
        return;
    }
    let (name, down) = match sort.strip_prefix('-') {
        Some(name) => (name, true),
        None => (sort, false),
    };
    let Some(at) = fields.iter().position(|field| field.name == name) else {
        return;
    };
    rows.sort_by(|one, other| {
        let order = compare(one.get(at), other.get(at));
        if down { order.reverse() } else { order }
    });
}

fn encode(params: &HashMap<String, String>, skip: &[&str]) -> String {
    let mut pairs: Vec<(&String, &String)> = params
        .iter()
        .filter(|(key, _)| !skip.contains(&key.as_str()))
        .collect();
    pairs.sort();
    serde_urlencoded::to_string(pairs).unwrap_or_default()
}

fn href(base: &str, extra: &str) -> String {
    if base.is_empty() {
        format!("?{extra}")
    } else {
        format!("?{base}&{extra}")
    }
}

fn filter_input(field: &Field, value: &str) -> String {
    let name = field.name;
    let label = format!(r#"<label for="filter-{name}">{name}</label>"#);
    let value = escape(value);
    match field.kind.flat() {
        Kind::Id | Kind::Optional(_) => String::new(),
        Kind::Str => {
            format!(
                r#"{label}<input id="filter-{name}" name="{name}" type="text" value="{value}">"#
            )
        }
        Kind::Int | Kind::DateTime | Kind::Float => {
            format!(
                r#"{label}<input id="filter-{name}" name="{name}" type="number" value="{value}">"#
            )
        }
        Kind::Bool => {
            let picked = |want: &str| if value == want { " selected" } else { "" };
            format!(
                r#"{label}<select id="filter-{name}" name="{name}"><option value="">Any</option><option value="1"{}>Yes</option><option value="0"{}>No</option></select>"#,
                picked("1"),
                picked("0")
            )
        }
    }
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
    for model in &registered {
        let href = format!("{base}/{}/", model.table);
        let rows = store
            .fetch(&format!("SELECT COUNT(*) FROM {}", model.table), &[])
            .await
            .unwrap_or_default();
        let count = match rows.first().and_then(|row| row.get(0)) {
            Some(Value::Int(number)) => number.to_string(),
            _ => "0".into(),
        };
        items.push(Entry {
            href,
            title: model.table.to_string(),
            count,
        });
    }
    render(Dashboard { entries: items })
}

async fn list<M: AdminModel>(
    repo: Repo<M>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, Error> {
    let fields = M::fields();
    let at = locate(&M::columns(), &fields);
    let find = locate(&M::search(), &fields);
    let query = params.get("q").cloned().unwrap_or_default().to_lowercase();
    let sort = params.get("sort").cloned().unwrap_or_default();
    let mut rows: Vec<Vec<Value>> = Vec::new();
    for model in repo.all().await? {
        let values = with_id(&model, &fields);
        if keep(&fields, &values, &query, &params, &find) {
            rows.push(values);
        }
    }
    let total = rows.len();
    sort_rows(&fields, &mut rows, &sort);
    let pages = total.div_ceil(PAGE);
    let page = params
        .get("page")
        .and_then(|page| page.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, pages.max(1));
    let start = (page - 1) * PAGE;
    let end = (start + PAGE).min(total);

    let bare = encode(&params, &["page", "sort"]);
    let kept = encode(&params, &["page"]);
    let columns = at
        .iter()
        .map(|&i| {
            let name = fields[i].name;
            let (marker, toggle) = if sort == name {
                ("▲", format!("-{name}"))
            } else if sort == format!("-{name}") {
                ("▼", name.to_string())
            } else {
                ("", name.to_string())
            };
            Column {
                name: name.to_string(),
                marker,
                href: href(&bare, &format!("sort={toggle}")),
            }
        })
        .collect();
    let mut items = Vec::new();
    for values in &rows[start..end] {
        items.push(Item {
            id: id_of(values, &fields),
            cells: at.iter().map(|&i| cell(values, i)).collect(),
        });
    }
    let filters = fields
        .iter()
        .filter(|field| field.kind != Kind::Id)
        .map(|field| {
            filter_input(
                field,
                params.get(field.name).map(String::as_str).unwrap_or(""),
            )
        })
        .collect();
    let prev = (page > 1).then(|| href(&kept, &format!("page={}", page - 1)));
    let next = (page < pages).then(|| href(&kept, &format!("page={}", page + 1)));
    render(List {
        title: M::table(),
        q: params.get("q").cloned().unwrap_or_default(),
        sort,
        filters,
        columns,
        rows: items,
        total,
        page,
        pages,
        prev,
        next,
    })
}

fn with_id<M: Model>(model: &M, fields: &[Field]) -> Vec<Value> {
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

async fn detail<M: AdminModel>(
    repo: Repo<M>,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    let model = repo.get(id).await?.ok_or(Error::NotFound)?;
    let fields = M::fields();
    let values = with_id(&model, &fields);
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

async fn show_new<M: AdminModel>(
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
) -> Result<Response, Error> {
    let fixed = M::readonly();
    let inputs = M::fields()
        .iter()
        .filter(|field| field.kind != Kind::Id)
        .map(|field| {
            if fixed.contains(&field.name) {
                locked(field, field.default.as_ref())
            } else {
                input(field, field.default.as_ref())
            }
        })
        .collect();
    render(FormView {
        title: format!("New {}", M::table()),
        model: M::table(),
        home: "../",
        up: "./",
        sub: "New".into(),
        token: token(&headers, guard),
        inputs,
        errors: Vec::new(),
    })
}

async fn create<M: AdminModel>(
    repo: Repo<M>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Form(map): Form<HashMap<String, String>>,
) -> Result<Response, Error> {
    let fields = M::fields();
    let fixed = M::readonly();
    let mut values = Vec::with_capacity(fields.len());
    let mut inputs = Vec::new();
    let mut problems = Vec::new();
    for field in &fields {
        if field.kind == Kind::Id {
            values.push(Value::int(0));
            continue;
        }
        if fixed.contains(&field.name) {
            inputs.push(locked(field, field.default.as_ref()));
            values.push(field.default.clone().unwrap_or(Value::Null));
            continue;
        }
        let raw = map.get(field.name).map(String::as_str).unwrap_or("");
        inputs.push(input_raw(field, raw));
        match value(field, map.get(field.name)) {
            Ok(value) => values.push(value),
            Err(fail) => {
                problems.push(fail.to_string());
                values.push(Value::Null);
            }
        }
    }
    if !problems.is_empty() {
        return render(FormView {
            title: format!("New {}", M::table()),
            model: M::table(),
            home: "../",
            up: "./",
            sub: "New".into(),
            token: token(&headers, guard),
            inputs,
            errors: problems,
        });
    }
    let mut model = M::from_row(&Row { values })?;
    repo.save(&mut model).await?;
    Ok(view::redirect(&back(&uri, 1)))
}

async fn show_edit<M: AdminModel>(
    repo: Repo<M>,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    let model = repo.get(id).await?.ok_or(Error::NotFound)?;
    let values = model.row();
    let mut slots = values.iter();
    let mut inputs = Vec::new();
    let fixed = M::readonly();
    for field in M::fields() {
        if field.kind == Kind::Id {
            continue;
        }
        let old = slots.next();
        if fixed.contains(&field.name) {
            inputs.push(locked(&field, old));
        } else {
            inputs.push(input(&field, old));
        }
    }
    render(FormView {
        title: format!("Edit {} {id}", M::table()),
        model: M::table(),
        home: "../../",
        up: "../",
        sub: format!("Edit {id}"),
        token: token(&headers, guard),
        inputs,
        errors: Vec::new(),
    })
}

async fn replace<M: AdminModel>(
    repo: Repo<M>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<i64>,
    Form(map): Form<HashMap<String, String>>,
) -> Result<Response, Error> {
    let current = repo.get(id).await?.ok_or(Error::NotFound)?;
    let fields = M::fields();
    let fixed = M::readonly();
    let have = current.row();
    let mut slots = have.iter();
    let mut values = vec![Value::int(id)];
    let mut inputs = Vec::new();
    let mut problems = Vec::new();
    for field in &fields {
        if field.kind == Kind::Id {
            continue;
        }
        let old = slots.next();
        if fixed.contains(&field.name) {
            inputs.push(locked(field, old));
            values.push(old.cloned().unwrap_or(Value::Null));
            continue;
        }
        let raw = map.get(field.name).map(String::as_str).unwrap_or("");
        inputs.push(input_raw(field, raw));
        match value(field, map.get(field.name)) {
            Ok(value) => values.push(value),
            Err(fail) => {
                problems.push(fail.to_string());
                values.push(old.cloned().unwrap_or(Value::Null));
            }
        }
    }
    if !problems.is_empty() {
        return render(FormView {
            title: format!("Edit {} {id}", M::table()),
            model: M::table(),
            home: "../../",
            up: "../",
            sub: format!("Edit {id}"),
            token: token(&headers, guard),
            inputs,
            errors: problems,
        });
    }
    let model = M::from_row(&Row { values })?;
    repo.update(&model).await?;
    Ok(view::redirect(&back(&uri, 1)))
}

async fn remove<M: AdminModel>(
    repo: Repo<M>,
    OriginalUri(uri): OriginalUri,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    repo.delete(id).await?;
    Ok(view::redirect(&back(&uri, 2)))
}
