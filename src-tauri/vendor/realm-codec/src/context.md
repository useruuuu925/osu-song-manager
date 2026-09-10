# realm-codec/src

- `format.rs` — Purpose: Realm v9/v24 binary format constants, node header parsing, and bit-level element extraction
- `lib.rs` — Purpose: Public API for parsing and writing Realm binary database files; ColumnType/value/table/file types
- `reader.rs` — Purpose: Read-only Realm file parser that traverses Groups into Tables with columnar Rows. Supports leaf-root tables (String PK) and inner-node cluster tree tables (UUID PK) via realm-core v24+ cluster layout. Handles B+ tree traversal, string leaf decoding, MULTIPLY f32/f64 leaf decoding, leaf-of-refs partitioning, and compact string layouts
- `write.rs` — Purpose: Write support: build Realm v9 binary files from scratch (not used by the CLI tool)
