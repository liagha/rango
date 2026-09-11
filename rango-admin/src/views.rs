use std::{collections::HashMap, sync::Arc};

use askama::Template;
use axum::{
    extract::{Extension, Form, OriginalUri, Path, Query as Params},
    http::{HeaderMap, Uri},
};
use rango_authentication::Current;
use rango_core::{
    Error, Repository, Response, Row, Store, Value,
    forgery::{Token, cookie},
    model::{
        Action as Deed, Filter, Key, Model, Name, Only, Op, Order, Page, Query, Schema, Sort,
        Table, Tree, Type, key, many,
    },
    view::{self, render},
};

use super::form::{filter_input, input, input_raw, locked, value};
use super::history::{Action, History, log};
use super::query::{PAGE, encode, here, href, tree};
use super::row::{align, cell, id_of, locate, text, when, with_id};

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
    title: Table,
    q: String,
    sort: String,
    query: String,
    held: Vec<Held>,
    notice: String,
    actions: Vec<Deed>,
    token: String,
    filters: Vec<String>,
    columns: Vec<Column>,
    rows: Vec<Item>,
    total: usize,
    page: usize,
    pages: usize,
    lo: usize,
    hi: usize,
    first: Option<String>,
    prev: Option<String>,
    next: Option<String>,
    last: Option<String>,
}

struct Held {
    name: String,
    value: String,
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
    model: Table,
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
    title: Table,
    id: String,
    notice: String,
    pairs: Vec<Pair>,
    past: Vec<Log>,
    inlines: Vec<Inline>,
    query: String,
    token: String,
}

struct Pair {
    name: String,
    value: String,
}

struct Inline {
    title: String,
    columns: Vec<String>,
    rows: Vec<Item>,
    href: String,
}

struct Log {
    at: String,
    user: String,
    action: String,
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

pub(crate) async fn dashboard(
    models: Extension<Arc<Vec<Schema>>>,
    store: Extension<Arc<dyn Store>>,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, Error> {
    let base = uri.path().trim_end_matches('/');
    let mut items = Vec::new();
    for model in models.0.iter() {
        let href = format!("{base}/{}/", model.table);
        let _ = store.define(model).await;
        let total = store
            .total_query(
                model,
                &Query {
                    tree: Tree::And(Vec::new()),
                    sort: Vec::new(),
                    page: Page::all(),
                    only: Only::All,
                    mass: None,
                },
            )
            .await
            .unwrap_or_default();
        items.push(Entry {
            href,
            title: model.table.to_string(),
            count: total.to_string(),
        });
    }
    render(Dashboard { entries: items })
}

pub(crate) async fn list<M: Model>(
    repository: Repository<M>,
    store: Extension<Arc<dyn Store>>,
    models: Extension<Arc<Vec<Schema>>>,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Params(mut params): Params<HashMap<String, String>>,
) -> Result<Response, Error> {
    let fields = M::fields();
    let at = locate(&M::columns(), &fields);
    let find = locate(&M::search(), &fields);
    let query = params.get("q").cloned().unwrap_or_default().to_lowercase();
    let sort = params.get("sort").cloned().unwrap_or_default();
    let notice = if let Some(note) = params.remove("notice") {
        note
    } else if params.remove("saved").is_some() {
        "Saved.".into()
    } else if params.remove("deleted").is_some() {
        "Deleted.".into()
    } else {
        String::new()
    };
    let mut related: HashMap<usize, Vec<String>> = HashMap::new();
    if !query.is_empty() {
        for (i, field) in fields.iter().enumerate() {
            let Some((table, _)) = field.reference() else {
                continue;
            };
            let Some(other) = models.0.iter().find(|spec| spec.table == table) else {
                continue;
            };
            let display: Vec<Tree> = other
                .fields
                .iter()
                .filter(|field| matches!(field.kind.flat(), Type::Str))
                .map(|field| {
                    Tree::Leaf(Filter {
                        field: field.name,
                        op: Op::Like,
                        value: Value::str(format!("%{query}%")),
                    })
                })
                .collect();
            if display.is_empty() {
                continue;
            }
            let rows = store
                .scan_query(
                    other,
                    &Query {
                        tree: Tree::Or(display),
                        sort: Vec::new(),
                        page: Page::all(),
                        only: Only::All,
                        mass: None,
                    },
                )
                .await
                .unwrap_or_default();
            let pk = other
                .fields
                .iter()
                .position(|field| matches!(field.kind.flat(), Type::Id | Type::Key))
                .unwrap_or(0);
            related.insert(i, rows.iter().map(|row| text(row.values.get(pk))).collect());
        }
    }
    let pick = tree(&fields, &params, &query, &find, &related);
    let schema = M::schema();
    let total = repository
        .total_query(&Query {
            tree: pick,
            sort: Vec::new(),
            page: Page::all(),
            only: Only::All,
            mass: None,
        })
        .await?;
    let pages = total.div_ceil(PAGE);
    let page = params
        .get("page")
        .and_then(|page| page.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, pages.max(1));
    let offset = (page - 1) * PAGE;
    let rows: Vec<Vec<Value>> = repository
        .scan_query(&Query {
            tree: tree(&fields, &params, &query, &find, &related),
            sort: vec![Sort::parse(&sort, &schema)],
            page: Page {
                count: PAGE,
                offset,
            },
            only: Only::All,
            mass: None,
        })
        .await?
        .iter()
        .map(|model| with_id(model, &fields))
        .collect();

    let bare = encode(&params, &["page", "sort"]);
    let kept = encode(&params, &["page"]);
    let head = |name: Name| {
        let (marker, toggle) = if sort == name.as_str() {
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
    };
    let mut columns: Vec<Column> = at.iter().map(|&i| head(fields[i].name)).collect();
    if !columns
        .iter()
        .any(|column| column.name == schema.key().as_str())
    {
        columns.insert(0, head(schema.key()));
    }
    let mut items = Vec::new();
    for values in &rows {
        items.push(Item {
            id: id_of(values, &fields),
            cells: at.iter().map(|&i| cell(values, i)).collect(),
        });
    }
    let filters = fields
        .iter()
        .filter(|field| field.kind != Type::Id && !many(&field.kind))
        .map(|field| {
            filter_input(
                field,
                params
                    .get(field.name.as_str())
                    .map(String::as_str)
                    .unwrap_or(""),
            )
        })
        .collect();
    let mut held: Vec<Held> = params
        .iter()
        .filter(|(name, _)| !["q", "sort", "page"].contains(&name.as_str()))
        .map(|(name, value)| Held {
            name: name.clone(),
            value: value.clone(),
        })
        .collect();
    held.sort_by(|left, right| left.name.cmp(&right.name));
    let prev = (page > 1).then(|| href(&kept, &format!("page={}", page - 1)));
    let next = (page < pages).then(|| href(&kept, &format!("page={}", page + 1)));
    let first = (page > 1).then(|| href(&kept, "page=1"));
    let last = (page < pages).then(|| href(&kept, &format!("page={pages}")));
    let lo = if total == 0 { 0 } else { offset + 1 };
    let hi = (offset + PAGE).min(total);
    render(List {
        title: M::table(),
        q: params.get("q").cloned().unwrap_or_default(),
        sort,
        query: here(&params),
        held,
        notice,
        actions: M::actions(),
        token: token(&headers, guard),
        filters,
        columns,
        rows: items,
        total,
        page,
        pages,
        lo,
        hi,
        first,
        prev,
        next,
        last,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn detail<M: Model>(
    repository: Repository<M>,
    history: Repository<History>,
    store: Extension<Arc<dyn Store>>,
    models: Extension<Arc<Vec<Schema>>>,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<String>,
    Params(mut params): Params<HashMap<String, String>>,
) -> Result<Response, Error> {
    let model = repository
        .get(&key::<M>(&id))
        .await?
        .ok_or(Error::NotFound)?;
    let notice = if let Some(note) = params.remove("notice") {
        note
    } else if params.remove("saved").is_some() {
        "Saved.".into()
    } else if params.remove("deleted").is_some() {
        "Deleted.".into()
    } else {
        String::new()
    };
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
    let mut events = Vec::new();
    if let Ok(all) = history.all().await {
        for event in all {
            if event.model.as_str() == M::table().as_str() && event.row == id {
                events.push(event);
            }
        }
    }
    events.sort_by_key(|event| std::cmp::Reverse(event.at));
    let past: Vec<Log> = events
        .into_iter()
        .map(|event| Log {
            at: when(&event.at),
            user: event.user,
            action: event.action.name().to_string(),
        })
        .collect();
    let mut inlines = Vec::new();
    for other in models.0.iter() {
        if other.table == M::table() {
            continue;
        }
        for field in &other.fields {
            let linked = field
                .reference()
                .is_some_and(|(table, _)| table == M::table());
            if !linked {
                continue;
            }
            let rows = store
                .scan_query(
                    other,
                    &Query {
                        tree: Tree::Leaf(Filter {
                            field: field.name,
                            op: Op::Eq,
                            value: key::<M>(&id),
                        }),
                        sort: vec![Sort {
                            field: other.key(),
                            order: Order::Asc,
                        }],
                        page: Page::all(),
                        only: Only::All,
                        mass: None,
                    },
                )
                .await
                .unwrap_or_default();
            let at: Vec<usize> = other
                .fields
                .iter()
                .enumerate()
                .filter(|(_, field)| {
                    !matches!(field.kind.flat(), Type::Id | Type::Key) && !many(&field.kind)
                })
                .map(|(i, _)| i)
                .collect();
            let mut items = Vec::new();
            for row in &rows {
                let values = align(&row.values, &other.fields);
                items.push(Item {
                    id: id_of(&values, &other.fields),
                    cells: at.iter().map(|&i| cell(&values, i)).collect(),
                });
            }
            inlines.push(Inline {
                title: other.table.to_string(),
                columns: std::iter::once(other.key().to_string())
                    .chain(at.iter().map(|&i| other.fields[i].name.to_string()))
                    .collect(),
                rows: items,
                href: format!("../../{}/?{}={}", other.table, field.name, id),
            });
        }
    }
    render(Detail {
        title: M::table(),
        id,
        notice,
        pairs,
        past,
        inlines,
        query: here(&params),
        token: token(&headers, guard),
    })
}

pub(crate) async fn show_new<M: Model>(
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

pub(crate) async fn create<M: Model>(
    repository: Repository<M>,
    history: Repository<History>,
    current: Current,
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
        let raw = map
            .get(field.name.as_str())
            .map(String::as_str)
            .unwrap_or("");
        inputs.push(input_raw(field, raw));
        match value(field, map.get(field.name.as_str())) {
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
    log(&history, M::table(), &model.id(), Action::Create, &current).await;
    Ok(view::redirect(&format!("{}?saved=1", back(&uri, 1))))
}

pub(crate) async fn show_edit<M: Model>(
    repository: Repository<M>,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    let model = repository
        .get(&key::<M>(&id))
        .await?
        .ok_or(Error::NotFound)?;
    let values = model.row();
    let mut slots = values.iter();
    let mut inputs = Vec::new();
    let fixed = M::readonly();
    for field in M::fields() {
        if field.kind == Type::Id {
            continue;
        }
        let old = slots.next();
        if fixed.contains(&field.name) || matches!(field.kind.flat(), Type::Key) {
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn replace<M: Model>(
    repository: Repository<M>,
    history: Repository<History>,
    current: Current,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    guard: Option<Extension<Token>>,
    Path(id): Path<String>,
    Form(map): Form<HashMap<String, String>>,
) -> Result<Response, Error> {
    let saved = repository
        .get(&key::<M>(&id))
        .await?
        .ok_or(Error::NotFound)?;
    let fields = M::fields();
    let fixed = M::readonly();
    let have = saved.row();
    let mut slots = have.iter();
    let mut values = vec![saved.id()];
    let mut inputs = Vec::new();
    let mut problems = Vec::new();
    for field in &fields {
        if field.kind == Type::Id {
            continue;
        }
        let old = slots.next();
        if matches!(field.kind.flat(), Type::Key) {
            continue;
        }
        if fixed.contains(&field.name) {
            inputs.push(locked(field, old));
            values.push(old.cloned().unwrap_or(Value::Null));
            continue;
        }
        let raw = map
            .get(field.name.as_str())
            .map(String::as_str)
            .unwrap_or("");
        inputs.push(input_raw(field, raw));
        match value(field, map.get(field.name.as_str())) {
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
    log(&history, M::table(), &key::<M>(&id), Action::Edit, &current).await;
    Ok(view::redirect(&format!("{}?saved=1", back(&uri, 1))))
}

pub(crate) async fn remove<M: Model>(
    repository: Repository<M>,
    history: Repository<History>,
    current: Current,
    OriginalUri(uri): OriginalUri,
    Path(id): Path<String>,
) -> Result<Response, Error> {
    repository.delete(&key::<M>(&id)).await?;
    log(
        &history,
        M::table(),
        &key::<M>(&id),
        Action::Delete,
        &current,
    )
    .await;
    Ok(view::redirect(&format!("{}?deleted=1", back(&uri, 2))))
}

pub(crate) async fn act<M: Model>(
    store: Extension<Arc<dyn Store>>,
    history: Repository<History>,
    current: Current,
    OriginalUri(uri): OriginalUri,
    body: String,
) -> Result<Response, Error> {
    let schema = M::schema();
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(&body).unwrap_or_default();
    let field = |key: &str| {
        pairs
            .iter()
            .find(|pair| pair.0 == key)
            .map(|pair| pair.1.as_str())
            .unwrap_or("")
    };
    let base = back(&uri, 1);
    let kept = field("back").trim_start_matches('?');
    let to = |msg: &str| {
        let extra = serde_urlencoded::to_string([("notice", msg)]).unwrap_or_default();
        let to = if kept.is_empty() {
            format!("{base}?{extra}")
        } else {
            format!("{base}?{kept}&{extra}")
        };
        view::redirect(&to)
    };
    let name = field("action");
    let Some(deed) = M::actions()
        .into_iter()
        .find(|deed| deed.name.as_str() == name)
    else {
        return Ok(to("Unknown action."));
    };
    let keys: Vec<Key> = pairs
        .iter()
        .filter(|pair| pair.0 == "ids")
        .map(|pair| Key::parse(&pair.1, &schema))
        .collect();
    if keys.is_empty() {
        return Ok(to("No rows selected."));
    }
    let msg = (deed.run)(store.0.clone(), schema, keys.clone()).await?;
    if deed.logged {
        for key in &keys {
            log(&history, M::table(), &key.value(), Action::Delete, &current).await;
        }
    }
    Ok(to(&msg))
}
