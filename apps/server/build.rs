// sqlx::migrate!() embeds the migration files at compile time but does not
// notice a newly added file on its own, so a build could ship (or a test could
// check) a stale set. Rebuild whenever the migrations directory changes.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
