use std::{
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
    Error, Repository, Response, Row, Store, Value,
    forgery::{Token, cookie},
    model::{Model, Type},
    urls::Routes,
    view::{self, render},
};

mod form;
mod query;
mod row;

use form::{filter_input, input, input_raw, locked, value};
use query::{PAGE, encode, href, keep, sort_rows};
use row::{cell, id_of, locate, with_id};

#[derive(Clone)]
struct Registered {
    table: &'static str,
}

pub trait AdminModel: Model {
    fn columns() -> Vec<&'static str> {
        Self::fields()
            .iter()
            .filter(|field| field.kind != Type::Id)
            .map(|field| field.name)
            .collect()
    }

    fn search() -> Vec<&'static str> {
        Self::fields()
            .iter()
            .filter(|field| matches!(field.kind.flat(), Type::Str))
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
    repository: Repository<M>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, Error> {
    let fields = M::fields();
    let at = locate(&M::columns(), &fields);
    let find = locate(&M::search(), &fields);
    let query = params.get("q").cloned().unwrap_or_default().to_lowercase();
    let sort = params.get("sort").cloned().unwrap_or_default();
    let mut rows: Vec<Vec<Value>> = Vec::new();
    for model in repository.all().await? {
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
        .filter(|field| field.kind != Type::Id)
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

async fn detail<M: AdminModel>(
    repository: Repository<M>,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    let model = repository.get(id).await?.ok_or(Error::NotFound)?;
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
        .filter(|field| field.kind != Type::Id)
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
    repository: Repository<M>,
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
        if field.kind == Type::Id {
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
    repository.save(&mut model).await?;
    Ok(view::redirect(&back(&uri, 1)))
}

async fn show_edit<M: AdminModel>(
    repository: Repository<M>,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    let model = repository.get(id).await?.ok_or(Error::NotFound)?;
    let values = model.row();
    let mut slots = values.iter();
    let mut inputs = Vec::new();
    let fixed = M::readonly();
    for field in M::fields() {
        if field.kind == Type::Id {
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
    repository: Repository<M>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<i64>,
    Form(map): Form<HashMap<String, String>>,
) -> Result<Response, Error> {
    let current = repository.get(id).await?.ok_or(Error::NotFound)?;
    let fields = M::fields();
    let fixed = M::readonly();
    let have = current.row();
    let mut slots = have.iter();
    let mut values = vec![Value::int(id)];
    let mut inputs = Vec::new();
    let mut problems = Vec::new();
    for field in &fields {
        if field.kind == Type::Id {
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
    repository.update(&model).await?;
    Ok(view::redirect(&back(&uri, 1)))
}

async fn remove<M: AdminModel>(
    repository: Repository<M>,
    OriginalUri(uri): OriginalUri,
    Path(id): Path<i64>,
) -> Result<Response, Error> {
    repository.delete(id).await?;
    Ok(view::redirect(&back(&uri, 2)))
}
