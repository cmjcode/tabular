# WebAssembly (Wasm) Plugin Development Guide

Tabular features a sandboxed, pure-Rust WebAssembly plugin runtime powered by the `wasmi` interpreter engine (located in `src/plugin_runtime/`).

Plugins can read selected table data, inspect schemas, generate custom export formats, or produce ORM source models without needing to recompile Tabular.

---

## 🏗️ Architecture & Host API

Wasm guest modules communicate with the Tabular Host via standard C-ABI functions:

```
+-----------------------------------------------------------------------------------------+
|                                    TABULAR HOST                                         |
+-----------------------------------------------------------------------------------------+
|  Host APIs:                                                                             |
|  • tabular_get_selected_rows_len() -> i32                                               |
|  • tabular_get_selected_rows_data(ptr: i32, max_len: i32) -> i32                        |
|  • tabular_get_table_schema_len() -> i32                                                |
|  • tabular_get_table_schema_data(ptr: i32, max_len: i32) -> i32                         |
|  • tabular_set_result(ptr: i32, len: i32)                                               |
|  • tabular_log(level: i32, ptr: i32, len: i32)                                          |
+-----------------------------------------------------------------------------------------+
                                      ▲              │
                               Memory │ Buffer       │ Function Calls
                                      │              ▼
+-----------------------------------------------------------------------------------------+
|                               WASM SANDBOX GUEST MODULE                                 |
|                         (Compiled from Rust, C, WAT, or Zig)                            |
+-----------------------------------------------------------------------------------------+
```

---

## 🚀 Building a Plugin in Rust

### 1. Configure `Cargo.toml`
```toml
[package]
name = "my_custom_exporter"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### 2. Implement the Entrypoint
```rust
extern "C" {
    fn tabular_get_selected_rows_len() -> i32;
    fn tabular_get_selected_rows_data(ptr: *mut u8, max_len: i32) -> i32;
    fn tabular_set_result(ptr: *const u8, len: i32);
    fn tabular_log(level: i32, ptr: *const u8, len: i32);
}

#[no_mangle]
pub extern "C" fn tabular_plugin_run() -> i32 {
    let len = unsafe { tabular_get_selected_rows_len() };
    if len <= 0 {
        return 0;
    }

    let mut buf = vec![0u8; len as usize];
    unsafe {
        tabular_get_selected_rows_data(buf.as_mut_ptr(), len);
    }

    // Process JSON data
    let json_str = String::from_utf8_lossy(&buf);
    let output = format!("// Generated from Tabular\n// Records count: {}\n{}", len, json_str);

    unsafe {
        tabular_set_result(output.as_ptr(), output.len() as i32);
    }

    1
}
```

### 3. Compile to WebAssembly
```bash
cargo build --target wasm32-unknown-unknown --release
```

The resulting `.wasm` binary in `target/wasm32-unknown-unknown/release/` can be loaded directly into Tabular via the **Plugin Manager Modal**.

---

## 📦 Built-In Starter Plugins & Templates

Tabular includes pre-configured plugins ready to run from `src/plugin_runtime/templates/`:
1. **Apache Parquet / DuckDB Exporter**: Converts result sets to column-oriented formats.
2. **Rust ORM Generator**: Emits `Diesel` and `SeaORM` entity definitions with typed fields.
3. **TypeScript ORM Generator**: Emits `Prisma` schema models and `TypeORM` entities.
4. **Python ORM Generator**: Emits `SQLAlchemy 2.0` Declarative Base models.
