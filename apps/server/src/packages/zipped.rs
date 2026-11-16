//! The zip form of a package (M1 section 3.6, read since M1.11): a
//! `package.json` at the root (or in the one folder everything sits in),
//! naming a `path` for each part. Reading it gives the inline form, every
//! part's body in place, which is what preview and install take.
//!
//! A part's file is JSON, except pages (`pages/<key>.md`: Markdown with
//! front matter, `title` and `guide`) and skills (`skills/<name>/SKILL.md`:
//! front matter `name` and `description`, then the instructions). Sizes are
//! capped before anything is read, so a small zip can't unpack into a large
//! one.

use std::io::{Cursor, Read};

use serde_json::{Map, Value, json};

/// The most bytes of a zipped package.
pub const MAX_ZIP_BYTES: usize = 5 * 1024 * 1024;
/// The most bytes it unpacks to, all files together.
const MAX_UNPACKED_BYTES: u64 = 5 * 1024 * 1024;
/// The most files in it.
const MAX_FILES: usize = 400;

/// Whether `bytes` look like a zip.
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
}

/// The inline form of a zipped package, or what's wrong with it.
pub fn inline(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_ZIP_BYTES {
        return Err("A zipped package is at most 5 MB.".to_owned());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| "This isn't a zip file that can be read.".to_owned())?;
    if archive.len() > MAX_FILES {
        return Err(format!("A zipped package holds at most {MAX_FILES} files."));
    }
    let mut declared = 0u64;
    let mut names = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let file = archive
            .by_index(index)
            .map_err(|_| "A file in the zip can't be read.".to_owned())?;
        declared = declared.saturating_add(file.size());
        if !file.is_dir() {
            let name = file
                .name()
                .map_err(|_| "A file in the zip has a name that can't be read.".to_owned())?;
            names.push(name.into_owned());
        }
    }
    if declared > MAX_UNPACKED_BYTES {
        return Err("The zip unpacks to more than 5 MB.".to_owned());
    }
    // Everything may sit in one folder, as zipping a folder makes it.
    let prefix = if names.iter().any(|name| name == "package.json") {
        String::new()
    } else {
        let candidates = names
            .iter()
            .filter(|name| name.matches('/').count() == 1 && name.ends_with("/package.json"))
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [one] => one.trim_end_matches("package.json").to_owned(),
            _ => return Err("The zip has no package.json at its root.".to_owned()),
        }
    };
    let mut read = |path: &str| -> Result<String, String> {
        let name = format!("{prefix}{path}");
        let mut file = archive
            .by_name(&name)
            .map_err(|_| format!("The zip has no {path}."))?;
        let mut text = String::new();
        file.by_ref()
            .take(MAX_UNPACKED_BYTES)
            .read_to_string(&mut text)
            .map_err(|_| format!("{path} isn't UTF-8 text."))?;
        Ok(text)
    };
    let manifest = read("package.json")?;
    let mut document = serde_json::from_str::<Value>(&manifest)
        .map_err(|error| format!("package.json isn't JSON: {error}"))?;
    let Some(contents) = document
        .get_mut("contents")
        .and_then(Value::as_array_mut)
    else {
        return Err("package.json lists no contents.".to_owned());
    };
    for part in contents.iter_mut() {
        let Some(map) = part.as_object_mut() else {
            continue;
        };
        let Some(path) = map.get("path").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        if path.starts_with('/') || path.split('/').any(|step| step == "..") {
            return Err(format!("{path}: a part's path stays inside the package."));
        }
        let text = read(&path)?;
        let kind = map.get("kind").and_then(Value::as_str).unwrap_or("");
        let body = match kind {
            "page" if path.ends_with(".md") => page(&text),
            "skill" if path.ends_with(".md") => skill(&text),
            _ => serde_json::from_str::<Value>(&text)
                .map_err(|error| format!("{path} isn't JSON: {error}"))?,
        };
        map.remove("path");
        map.insert("body".to_owned(), body);
    }
    // The zip's digest covered its own files; the inline form gets its own
    // when it is stored.
    if let Some(map) = document.as_object_mut() {
        map.remove("digest");
    }
    Ok(document)
}

/// Front matter (`---` lines of `key: value`) and the text after it.
fn front_matter(text: &str) -> (Map<String, Value>, String) {
    let mut fields = Map::new();
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (fields, text.to_owned());
    };
    let mut body = String::new();
    let mut in_matter = true;
    for line in rest.split_inclusive('\n') {
        if in_matter {
            let trimmed = line.trim_end();
            if trimmed == "---" {
                in_matter = false;
                continue;
            }
            if let Some((key, value)) = trimmed.split_once(':') {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                let value = match value {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    other => Value::String(other.to_owned()),
                };
                fields.insert(key.trim().to_owned(), value);
            }
        } else {
            body.push_str(line);
        }
    }
    if in_matter {
        return (Map::new(), text.to_owned());
    }
    (fields, body.trim_start_matches(['\r', '\n']).to_owned())
}

fn page(text: &str) -> Value {
    let (fields, markdown) = front_matter(text);
    let mut body = json!({
        "title": fields.get("title").cloned().unwrap_or(Value::String(String::new())),
        "markdown": markdown,
    });
    if let Some(guide) = fields.get("guide") {
        body["guide"] = guide.clone();
    }
    if let Some(template) = fields.get("template") {
        body["template"] = template.clone();
    }
    body
}

fn skill(text: &str) -> Value {
    let (fields, instructions) = front_matter(text);
    json!({
        "name": fields.get("name").cloned().unwrap_or(Value::String(String::new())),
        "description": fields
            .get("description")
            .cloned()
            .unwrap_or(Value::String(String::new())),
        "instructions": instructions,
    })
}

/// A zip of `files` (path, text), for tests.
#[cfg(test)]
pub(crate) fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;
    let mut buffer = Cursor::new(Vec::new());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zip_reads_into_the_inline_form() {
        let manifest = json!({
            "format": "agent-workspace-package/1",
            "id": "studio.notes",
            "version": "1.0.0",
            "name": "Notes",
            "target": "space",
            "contents": [
                {"kind": "collection", "key": "ideas", "path": "collections/ideas.json"},
                {"kind": "page", "key": "readme", "path": "pages/readme.md"},
                {"kind": "skill", "key": "triage", "path": "skills/triage/SKILL.md"}
            ],
            "digest": "sha256:stale"
        });
        let bytes = zip_of(&[
            ("notes/package.json", &manifest.to_string()),
            (
                "notes/collections/ideas.json",
                r#"{"name": "Ideas", "fields": []}"#,
            ),
            (
                "notes/pages/readme.md",
                "---\ntitle: Read me\nguide: true\n---\n\n# Hello\n",
            ),
            (
                "notes/skills/triage/SKILL.md",
                "---\nname: Triage\ndescription: Sort what comes in\n---\nSort it.\n",
            ),
        ]);
        let document = inline(&bytes).expect("reads");
        assert!(document.get("digest").is_none());
        let contents = document["contents"].as_array().expect("contents");
        assert_eq!(contents[0]["body"]["name"], "Ideas");
        assert!(contents[0].get("path").is_none());
        assert_eq!(contents[1]["body"]["title"], "Read me");
        assert_eq!(contents[1]["body"]["guide"], true);
        assert_eq!(contents[1]["body"]["markdown"], "# Hello\n");
        assert_eq!(contents[2]["body"]["description"], "Sort what comes in");
        assert_eq!(contents[2]["body"]["instructions"], "Sort it.\n");
        let package =
            serde_json::from_value::<super::super::format::Package>(document).expect("a package");
        package.validate().expect("valid");
    }

    #[test]
    fn a_broken_zip_says_why() {
        assert!(inline(b"PK\x03\x04 nope").is_err());
        let no_manifest = zip_of(&[("readme.txt", "hi")]);
        assert!(inline(&no_manifest).unwrap_err().contains("package.json"));
        let escaping = zip_of(&[(
            "package.json",
            r#"{"contents": [{"kind": "page", "key": "x", "path": "../x.md"}]}"#,
        )]);
        assert!(inline(&escaping).unwrap_err().contains("inside the package"));
    }
}
