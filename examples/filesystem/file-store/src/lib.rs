wit_bindgen::generate!({
    path: "../wit",
    world: "file-store",
    generate_all
});

use wasi::filesystem::types::{Descriptor, DescriptorFlags, DescriptorType, OpenFlags, PathFlags};

struct FileStore;

fn preopen() -> Result<Descriptor, String> {
    wasi::filesystem::preopens::get_directories()
        .into_iter()
        .next()
        .map(|(descriptor, _path)| descriptor)
        .ok_or_else(|| "no preopened directory".to_string())
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

        // A p3 stream is paired with a future carrying the final result:
        // drain the stream, then check the future for an error.
        let (mut stream, result) = file.read_via_stream(0);
        let mut contents = Vec::new();
        while let Some(byte) = stream.next().await {
            contents.push(byte);
        }
        result
            .await
            .map_err(|e| format!("cannot read '{name}': {e}"))?;

        String::from_utf8(contents).map_err(|e| format!("'{name}' is not valid UTF-8: {e}"))
    }

    async fn write(name: String, contents: String) -> Result<u64, String> {
        // Fails with `not-permitted` unless the preopen grants "read-write".
        let file = open(
            &name,
            DescriptorFlags::WRITE,
            OpenFlags::CREATE | OpenFlags::TRUNCATE,
        )
        .await?;

        let bytes = contents.into_bytes();
        let written = bytes.len() as u64;

        let (mut tx, rx) = wit_stream::new();
        let result = file.write_via_stream(rx, 0);
        tx.write_all(bytes).await;
        drop(tx);
        result
            .await
            .map_err(|e| format!("cannot write '{name}': {e}"))?;

        Ok(written)
    }

    async fn list_files() -> Result<Vec<String>, String> {
        let (mut entries, result) = preopen()?.read_directory();

        let mut names = Vec::new();
        while let Some(entry) = entries.next().await {
            if matches!(entry.type_, DescriptorType::RegularFile) {
                names.push(entry.name);
            }
        }
        result
            .await
            .map_err(|e| format!("cannot read directory: {e}"))?;

        names.sort();
        Ok(names)
    }
}

export!(FileStore);
