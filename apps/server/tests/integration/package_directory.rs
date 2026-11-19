//! M1.11: the package directory. The silent failures: a package dropped in
//! the server's folder that never shows up or can't be installed, a broken
//! one that disappears instead of saying why, a zip whose parts don't
//! arrive, and one workspace's list showing in another.

use std::io::Write as _;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use super::support::{
    callers::{Call, Caller},
    world::{Workspace, WorldBuilder},
};

async fn tool(world: &Workspace, name: &'static str, arguments: Value) -> Value {
    world.apps["desk"]
        .call(&world.app, Call::Tool { name, arguments })
        .await
        .1
}

fn notes(id: &str, name: &str) -> Value {
    json!({
        "format": "agent-workspace-package/1",
        "id": id,
        "version": "1.0.0",
        "name": name,
        "description": "Somewhere to keep ideas",
        "target": "space",
        "contents": [
            {"kind": "space", "key": "notes", "body": {"name": name, "key": "NOTE"}},
            {"kind": "collection", "key": "ideas", "body": {
                "name": "Ideas", "titleName": "Idea",
                "fields": [{"name": "Why", "type": "text"}], "views": []}}
        ]
    })
}

fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        for (path, text) in files {
            writer
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .expect("start");
            writer.write_all(text.as_bytes()).expect("write");
        }
        writer.finish().expect("finish");
    }
    buffer.into_inner()
}

fn entries(listed: &Value) -> Vec<Value> {
    listed["value"]["entries"]
        .as_array()
        .unwrap_or_else(|| panic!("entries in {listed}"))
        .clone()
}

#[tokio::test]
async fn a_package_in_the_folder_is_listed_and_installs() {
    let folder = tempfile::tempdir().expect("folder");
    let data = tempfile::tempdir().expect("data root");
    std::fs::write(
        folder.path().join("notes.json"),
        notes("studio.notes", "Notes").to_string(),
    )
    .expect("notes");
    std::fs::write(folder.path().join("broken.json"), "{ not a package").expect("broken");
    let mut zipped = notes("studio.zipped", "Zipped notes");
    zipped["contents"][1] = json!({"kind": "collection", "key": "ideas", "path": "ideas.json"});
    std::fs::write(
        folder.path().join("zipped.zip"),
        zip_of(&[
            ("package.json", &zipped.to_string()),
            (
                "ideas.json",
                r#"{"name": "Ideas", "titleName": "Idea", "fields": [], "views": []}"#,
            ),
        ]),
    )
    .expect("zip");
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .config(
            "AGENT_WORKSPACE_DIRECTORY_DIR",
            folder.path().to_string_lossy(),
        )
        .config("AGENT_WORKSPACE_DATA_ROOT", data.path().to_string_lossy())
        .build("m111_directory_folder")
        .await
    else {
        return;
    };
    let read =
        agent_workspace_server::packages::directory::refresh(world.pool(), &world.state.config)
            .await
            .expect("refresh");
    assert_eq!(read, 1, "the one folder is read");

    let listed = tool(&world, "directory_list", json!({"shapes": false})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let found = entries(&listed);
    let by_id = |id: &str| {
        found
            .iter()
            .find(|entry| entry["id"] == id)
            .unwrap_or_else(|| panic!("{id} in {listed}"))
            .clone()
    };
    // Built in first, then what the folder holds; shapes aren't packages to install.
    assert_eq!(found[0]["source"], "built_in", "{listed}");
    assert!(found.iter().all(|entry| entry["shape"] != true), "{listed}");
    assert_eq!(by_id("studio.notes")["source"], "server");
    assert!(by_id("studio.notes")["error"].is_null());
    assert!(by_id("studio.zipped")["error"].is_null());
    let broken = by_id("broken");
    assert!(
        broken["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "a broken entry says why: {broken}"
    );
    let shapes = tool(&world, "directory_list", json!({"shapes": true})).await;
    assert!(
        entries(&shapes)
            .iter()
            .any(|entry| entry["id"] == "shapes.friends"),
        "{shapes}"
    );

    // Installing an entry: preview, then the plan it showed.
    let preview = tool(&world, "package_preview", json!({"entry": "studio.notes"})).await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let installed = tool(
        &world,
        "package_install",
        json!({"entry": "studio.notes", "planHash": preview["value"]["planHash"]}),
    )
    .await;
    assert_eq!(installed["outcome"], "ok", "{installed}");
    let (source, source_ref) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT source, source_ref FROM packages WHERE workspace_id = $1 AND package_key = 'studio.notes'",
    )
    .bind(world.organization)
    .fetch_one(world.pool())
    .await
    .expect("installed package");
    assert_eq!(source, "directory");
    assert!(source_ref.is_some());
    let ideas = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM collections c JOIN projects p ON p.id = c.project_id \
         WHERE p.organization_id = $1 AND c.name = 'Ideas'",
    )
    .bind(world.organization)
    .fetch_one(world.pool())
    .await
    .expect("collections");
    assert_eq!(ideas, 1);

    // The zipped one installs with its parts read from the zip.
    let preview = tool(&world, "package_preview", json!({"entry": "studio.zipped"})).await;
    assert_eq!(preview["outcome"], "ok", "{preview}");

    // A broken entry can't be previewed, and says so.
    let refused = tool(&world, "package_preview", json!({"entry": "broken"})).await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");

    // A zip uploaded as a file installs the same way.
    let task = tool(
        &world,
        "task_create",
        json!({"projectId": world.project("WORK"), "title": "Try the uploaded package"}),
    )
    .await;
    let key = task["value"]["key"].as_str().expect("task key").to_owned();
    let mut uploaded = notes("studio.uploaded", "Uploaded notes");
    uploaded["contents"][1] = json!({"kind": "page", "key": "readme", "path": "pages/readme.md"});
    let bytes = zip_of(&[
        ("uploaded/package.json", &uploaded.to_string()),
        (
            "uploaded/pages/readme.md",
            "---\ntitle: Read me\n---\n\n# Hello\n",
        ),
    ]);
    let file = tool(
        &world,
        "file_upload",
        json!({"taskId": key, "name": "uploaded.zip", "dataBase64": STANDARD.encode(&bytes)}),
    )
    .await;
    let file_id = file["value"]["fileId"]
        .as_str()
        .unwrap_or_else(|| panic!("{file}"));
    let preview = tool(&world, "package_preview", json!({"fileId": file_id})).await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let installed = tool(
        &world,
        "package_install",
        json!({"fileId": file_id, "planHash": preview["value"]["planHash"]}),
    )
    .await;
    assert_eq!(installed["outcome"], "ok", "{installed}");
    let page = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM project_pages pg JOIN projects p ON p.id = pg.project_id \
         WHERE p.organization_id = $1 AND pg.title = 'Read me'",
    )
    .bind(world.organization)
    .fetch_one(world.pool())
    .await
    .expect("pages");
    assert_eq!(page, 1);

    // Only one way to name the package.
    let both = tool(
        &world,
        "package_preview",
        json!({"entry": "studio.notes", "fileId": file_id}),
    )
    .await;
    assert_eq!(both["outcome"], "invalid_request", "{both}");
    world.finish().await;
}

#[tokio::test]
async fn a_saved_template_is_offered_as_a_package() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .build("m111_directory_saved")
        .await
    else {
        return;
    };
    let saved = tool(
        &world,
        "workspace_template_save",
        json!({"projectId": world.project("WORK"), "name": "Our setup", "description": "How we work"}),
    )
    .await;
    assert_eq!(saved["outcome"], "ok", "{saved}");
    let listed = tool(&world, "directory_list", json!({"query": "our setup"})).await;
    let found = entries(&listed);
    assert_eq!(found.len(), 1, "{listed}");
    assert_eq!(found[0]["source"], "saved_template");
    let entry = found[0]["entry"].as_str().expect("entry").to_owned();
    assert!(entry.starts_with("template:"), "{entry}");
    let preview = tool(&world, "package_preview", json!({"entry": entry})).await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    world.finish().await;
}
