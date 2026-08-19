wit_bindgen::generate!({
    path: "../wit",
    world: "file-store",
    generate_all
});

use wasi::filesystem::types::{Descriptor, DescriptorFlags, DescriptorType, OpenFlags, PathFlags};

struct FileStore;

// Matches the `guest` path in the config.
const DATA_DIR: &str = "/data";

// Each preopen directory is paired with its guest path.
fn preopen() -> Result<Descriptor, String> {
    wasi::filesystem::preopens::get_directories()
        .into_iter()
        .find(|(_descriptor, guest_path)| guest_path == DATA_DIR)
        .map(|(descriptor, _guest_path)| descriptor)
        .ok_or_else(|| format!("no preopened directory at '{DATA_DIR}'"))
}

async fn open(
    name: &str,
    flags: DescriptorFlags,
    open_flags: OpenFlags,
) -> Result<Descriptor, String> {
    preopen()?
        .open_at(PathFlags::empty(), name.to_string(), open_flags, flags)
        .await
        .map_err(|e| format!("cannot open '{name}': {e}"))
}

impl Guest for FileStore {
    async fn read(name: String) -> Result<String, String> {
        let file = open(&name, DescriptorFlags::READ, OpenFlags::empty()).await?;

        // Drain the stream from offset 0, then check the future for an error.
        let (stream, result) = file.read_via_stream(0);
        let contents = stream.collect().await;
        result
            .await
            .map_err(|e| format!("cannot read '{name}': {e}"))?;

        String::from_utf8(contents).map_err(|e| format!("'{name}' is not valid UTF-8: {e}"))
    }

    async fn write(name: String, contents: String) -> Result<u64, String> {
        // Open for write will fail if "read-write" perms were not granted.
        let file = open(
            &name,
            DescriptorFlags::WRITE,
            OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )
        .await?;

        let bytes = contents.into_bytes();
        let length = bytes.len() as u64;

        // TRUNCATE above emptied the file, so this writes from offset 0.
        let (mut tx, rx) = wit_stream::new();
        let result = file.write_via_stream(rx, 0);
        tx.write_all(bytes).await;

        // Dropping the writer closes the stream, which completes the future.
        drop(tx);
        result
            .await
            .map_err(|e| format!("cannot write '{name}': {e}"))?;

        Ok(length)
    }

    async fn list_files() -> Result<Vec<String>, String> {
        let (entries, result) = preopen()?.read_directory();
        let entries = entries.collect().await;
        result
            .await
            .map_err(|e| format!("cannot read directory: {e}"))?;

        let mut names: Vec<String> = entries
            .into_iter()
            .filter(|entry| matches!(entry.type_, DescriptorType::RegularFile))
            .map(|entry| entry.name)
            .collect();
        names.sort();
        Ok(names)
    }
}

export!(FileStore);
