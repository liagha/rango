rango_admin::manage!(
    helloworld::schema(),
    format!("{}/rango.sqlite", env!("CARGO_MANIFEST_DIR")),
    rango::store::sqlite::open
);
