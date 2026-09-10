// reader.rs
//
// Purpose: High-level Realm file reader that parses Groups into Tables with
// Rows of columnar Values.
//
// This module:
// - Parses the top-level Group array into named tables
// - Supports old-format column-based tables and new-format cluster tree tables
// - Handles string leaf nodes (compact, per-row refs, inline multiply)
// - Handles integer B+ trees for column value traversal
// - Allocates all parsed data eagerly into Vec<RealmTable>

use crate::format::{
    decode_short_string, multiply_elem_bytes, parse_file_header, read_bits_elem, NodeHeader,
    NODE_HEADER_SIZE, WTYPE_BITS, WTYPE_IGNORE, WTYPE_LINKLIST, WTYPE_MULTIPLY,
};
use crate::{ColumnType, RealmError, RealmTable, Result, Row, Value};

pub(crate) fn read_tables(data: &[u8]) -> Result<Vec<RealmTable>> {
    let (top_ref, _version) = parse_file_header(data)?;
    let group = read_array(data, top_ref)?;

    if group.len() < 2 {
        return Err(RealmError::InvalidFormat("group array too small".into()));
    }

    let names_ref = group[0] as usize;
    let tables_ref = group[1] as usize;

    let names = read_string_array_multiply(data, names_ref, 64)?;
    let table_refs = read_array(data, tables_ref)?;

    let count = names.len().min(table_refs.len());
    let mut tables = Vec::with_capacity(count);

    for i in 0..count {
        let tref = table_refs[i] as usize;
        if tref == 0 {
            continue;
        }
        if let Ok(t) = read_table(data, &names[i], tref) {
            tables.push(t);
        }
    }

    Ok(tables)
}

// ── Array primitives ──────────────────────────────────────────────────────────

/// Parse the array at `offset` and return its elements as `Vec<u64>`.
fn read_array(data: &[u8], offset: usize) -> Result<Vec<u64>> {
    read_array_inner(data, offset)
}

/// Exposed for diagnostic probing - reads an array at `offset` without
/// going through the table-level parser.
pub fn read_array_for_debug(data: &[u8], offset: usize) -> Result<Vec<u64>> {
    read_array_inner(data, offset)
}

fn read_array_inner(data: &[u8], offset: usize) -> Result<Vec<u64>> {
    if offset + NODE_HEADER_SIZE > data.len() {
        return Err(RealmError::InvalidFormat(format!(
            "array offset {offset:#x} out of bounds"
        )));
    }
    let hdr = NodeHeader::parse(data[offset..offset + 8].try_into().unwrap());
    let payload = &data[offset + NODE_HEADER_SIZE..];

    // Clamp to avoid panic from bogus leaf headers.
    let max_elems = match hdr.wtype {
        WTYPE_MULTIPLY if hdr.width as usize > 0 => (payload.len() / hdr.width as usize) as u64,
        _ if hdr.width > 0 => (payload.len() as u64 * 8) / hdr.width as u64,
        _ => 0,
    };
    let size = (hdr.size as u64).min(max_elems).min(10_000_000) as usize;

    let mut elems = Vec::with_capacity(size);
    match hdr.wtype {
        WTYPE_BITS => {
            for i in 0..size {
                elems.push(read_bits_elem(payload, i, hdr.width));
            }
        }
        WTYPE_MULTIPLY => {
            for i in 0..size {
                let slot = multiply_elem_bytes(payload, i, hdr.width);
                let val = match hdr.width {
                    8 => u64::from_le_bytes(slot.try_into().unwrap_or([0u8; 8])),
                    4 => u32::from_le_bytes(slot.try_into().unwrap_or([0u8; 4])) as u64,
                    2 => u16::from_le_bytes(slot.try_into().unwrap_or([0u8; 2])) as u64,
                    1 => slot[0] as u64,
                    _ => 0,
                };
                elems.push(val);
            }
        }
        _ => {}
    }
    Ok(elems)
}

fn read_string_array_multiply(data: &[u8], offset: usize, slot_width: u8) -> Result<Vec<String>> {
    read_string_array_multiply_inner(data, offset, slot_width)
}

/// Exposed for diagnostic probing - reads a multiply-encoded string array.
pub fn read_string_array_for_debug(
    data: &[u8],
    offset: usize,
    slot_width: u8,
) -> Result<Vec<String>> {
    read_string_array_multiply_inner(data, offset, slot_width)
}

fn read_string_array_multiply_inner(
    data: &[u8],
    offset: usize,
    slot_width: u8,
) -> Result<Vec<String>> {
    if offset + NODE_HEADER_SIZE > data.len() {
        return Err(RealmError::InvalidFormat(format!(
            "string array offset {offset:#x} out of bounds"
        )));
    }
    let hdr = NodeHeader::parse(data[offset..offset + 8].try_into().unwrap());
    let payload = &data[offset + NODE_HEADER_SIZE..];
    let mut result = Vec::with_capacity(hdr.size);
    for i in 0..hdr.size {
        let slot = multiply_elem_bytes(payload, i, slot_width);
        result.push(decode_short_string(slot));
    }
    Ok(result)
}

// ── Table ─────────────────────────────────────────────────────────────────────

fn read_table(data: &[u8], name: &str, table_ref: usize) -> Result<RealmTable> {
    let table_arr = read_array(data, table_ref)?;
    if table_arr.is_empty() {
        return Err(RealmError::InvalidFormat(format!(
            "table '{name}' array empty"
        )));
    }

    let spec_ref = table_arr[0] as usize;
    let spec_arr = read_array(data, spec_ref)?;
    if spec_arr.len() < 2 {
        return Err(RealmError::InvalidFormat(format!(
            "table '{name}' spec too small"
        )));
    }

    let slot_width = {
        let names_ref = spec_arr[1] as usize;
        if names_ref + 8 > data.len() {
            32
        } else {
            let nh = NodeHeader::parse(data[names_ref..names_ref + 8].try_into().unwrap());
            nh.width
        }
    };
    let col_names = read_string_array_multiply(data, spec_arr[1] as usize, slot_width)?;
    let col_type_ints = read_array(data, spec_arr[0] as usize)?;

    let columns: Vec<(String, ColumnType)> = col_names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let t = col_type_ints.get(i).copied().unwrap_or(0) as u8;
            (n.clone(), ColumnType::from_u8(t))
        })
        .collect();

    // v24+ cluster tree: cluster root at table_arr[2] (index 1 is reserved).
    if spec_arr.len() >= 4 {
        let col_attrs_raw = read_array(data, spec_arr[2] as usize).unwrap_or_default();
        let cluster_root_ref = if table_arr.len() > 2 && table_arr[2] != 0 {
            table_arr[2] as usize
        } else {
            table_arr[1] as usize
        };
        let cluster_root = read_array(data, cluster_root_ref)?;
        // realm-core v24+: root may be inner node ([key, depth, size, child0, ...])
        // vs leaf node ([pk_col, pk_index, col1, col2, ...]).
        // Detect inner root by reading the NodeHeader directly.
        let root_is_inner = cluster_root_ref + NODE_HEADER_SIZE <= data.len()
            && (data[cluster_root_ref + 4] & 0x80) != 0;
        if root_is_inner {
            return read_table_inner(
                data,
                name,
                &columns,
                &col_type_ints,
                &col_attrs_raw,
                &cluster_root,
            );
        }
        return read_table_new(
            data,
            name,
            &columns,
            &col_type_ints,
            &col_attrs_raw,
            &cluster_root,
        );
    }

    // Old format: table_arr = [spec_ref, col_ref_0, col_ref_1, ...]
    let col_refs: Vec<usize> = table_arr[1..].iter().map(|&r| r as usize).collect();

    let row_count = col_refs
        .iter()
        .find(|&&r| r != 0)
        .map(|&r| count_node_rows(data, r))
        .unwrap_or(0);

    let mut rows = Vec::with_capacity(row_count);
    for row_idx in 0..row_count {
        let values = col_refs
            .iter()
            .enumerate()
            .take(columns.len())
            .map(|(ci, &col_ref)| {
                let col_type = columns[ci].1;
                read_cell(data, col_ref, row_idx, col_type).unwrap_or(Value::Null)
            })
            .collect();
        rows.push(Row { values });
    }

    Ok(RealmTable {
        name: name.to_string(),
        columns,
        rows,
    })
}

// ── New-format (cluster tree) table reader ────────────────────────────────────

/// Build a `RealmTable` from a Realm SDK 5+ cluster-tree table with an inner root.
///
/// realm-core inner node layout:
///   `[key_ref (idx 0), sub_tree_depth (idx 1), sub_tree_size (idx 2),`
///    `child0 (idx 3), child1 (idx 4), ...]`
///
/// Each child is a leaf Cluster:
///   `[key_ref_or_size (idx 0), col0 (idx 1), col1 (idx 2), ...]`
///
/// Columns map 1:1 - leaf[1+k] → column k.
fn read_table_inner(
    data: &[u8],
    name: &str,
    columns: &[(String, ColumnType)],
    _col_type_ints: &[u64],
    col_attrs: &[u64],
    root_entries: &[u64],
) -> Result<RealmTable> {
    let n_children = root_entries.len().saturating_sub(3);
    let mut col_values: Vec<Vec<Value>> = vec![vec![]; columns.len()];

    for ci in 0..n_children {
        let child_ref = root_entries[3 + ci] as usize;
        if child_ref == 0
            || !child_ref.is_multiple_of(8)
            || child_ref + NODE_HEADER_SIZE > data.len()
        {
            continue;
        }
        let leaf = read_array(data, child_ref)?;
        if leaf.len() < 2 {
            continue;
        }

        let key_or_size = leaf[0];
        let leaf_rows = if (key_or_size & 1) != 0 {
            (key_or_size >> 1) as usize
        } else {
            let kref = key_or_size as usize;
            if kref > 0 && kref.is_multiple_of(8) && kref + NODE_HEADER_SIZE <= data.len() {
                read_array(data, kref).map(|a| a.len()).unwrap_or(0)
            } else {
                0
            }
        };
        if leaf_rows == 0 {
            continue;
        }

        for (col_idx, (_, col_type)) in columns.iter().enumerate() {
            let entry_idx = 1 + col_idx;
            if entry_idx >= leaf.len() {
                let cur_len = col_values[col_idx].len();
                col_values[col_idx].resize(cur_len + leaf_rows, Value::Null);
                continue;
            }
            let col_ref = leaf[entry_idx] as usize;
            let attr = col_attrs.get(col_idx).copied().unwrap_or(0);
            let is_list = (attr & 32) != 0;

            let mut vals: Vec<Value> = if col_ref == 0 {
                vec![Value::Null; leaf_rows]
            } else if is_list {
                collect_list_column_new(data, col_ref, *col_type, &[])
            } else {
                collect_cluster_column_new(data, col_ref, *col_type)
            };
            vals.resize(leaf_rows, Value::Null);
            vals.truncate(leaf_rows);
            col_values[col_idx].extend(vals);
        }
    }

    let row_count = col_values.iter().map(|v| v.len()).max().unwrap_or(0);
    let mut rows = Vec::with_capacity(row_count);
    for row_idx in 0..row_count {
        let values: Vec<Value> = col_values
            .iter()
            .map(|cv| cv.get(row_idx).cloned().unwrap_or(Value::Null))
            .collect();
        rows.push(Row { values });
    }

    Ok(RealmTable {
        name: name.to_string(),
        columns: columns.to_vec(),
        rows,
    })
}

/// Build a `RealmTable` from a Realm SDK 5+ cluster-tree table with a leaf root.
///
/// Cluster layout:
///   `cluster[0]`   = col\[0\] (primary key column - always a string)
///   `cluster[1]`   = pk_index B+ tree (skip - not a data column)
///   `cluster[2..]` = col\[1\], col\[2\], ...  with these exceptions:
///     type 8  (Timestamp) occupies 2 slots (seconds + fractionals)
///     type 13 (BackLink)  occupies 0 slots (virtual column, no cluster entry)
fn read_table_new(
    data: &[u8],
    name: &str,
    columns: &[(String, ColumnType)],
    col_type_ints: &[u64],
    col_attrs: &[u64],
    cluster_root: &[u64],
) -> Result<RealmTable> {
    // Determine true row count from the PK column's leaf
    let pk_ref = cluster_root.first().copied().unwrap_or(0) as usize;
    let parent_rows: Vec<u64> = if pk_ref == 0 {
        vec![]
    } else {
        collect_ints_new(data, pk_ref)
    };

    let mut col_values: Vec<Vec<Value>> = Vec::with_capacity(columns.len());

    for (col_idx, (_, col_type)) in columns.iter().enumerate() {
        let ci = cluster_index_for_col(col_idx, col_type_ints);
        let col_ref = cluster_root.get(ci).copied().unwrap_or(0) as usize;
        let attr = col_attrs.get(col_idx).copied().unwrap_or(0);
        let is_list = (attr & 32) != 0;

        let values: Vec<Value> = if col_ref == 0 {
            vec![]
        } else if col_idx == 0 {
            parent_rows.iter().map(|&v| Value::Int(v as i64)).collect()
        } else if is_list {
            collect_list_column_new(data, col_ref, *col_type, &parent_rows)
        } else {
            collect_cluster_column_new(data, col_ref, *col_type)
        };
        col_values.push(values);
    }

    let row_count = col_values.iter().map(|v| v.len()).max().unwrap_or(0);
    let mut rows = Vec::with_capacity(row_count);
    for row_idx in 0..row_count {
        let values: Vec<Value> = col_values
            .iter()
            .map(|cv| cv.get(row_idx).cloned().unwrap_or(Value::Null))
            .collect();
        rows.push(Row { values });
    }

    Ok(RealmTable {
        name: name.to_string(),
        columns: columns.to_vec(),
        rows,
    })
}

/// Collect a list-type column where the root partition mirrors parent row
/// count and each partition's sub-refs yield list elements per parent.
fn collect_list_column_new(
    data: &[u8],
    col_ref: usize,
    col_type: ColumnType,
    _parent_rows: &[u64],
) -> Vec<Value> {
    if col_ref == 0 || !col_ref.is_multiple_of(8) || col_ref + NODE_HEADER_SIZE > data.len() {
        return vec![];
    }
    let hdr = NodeHeader::parse(data[col_ref..col_ref + 8].try_into().unwrap());
    if hdr.is_inner || !(hdr.wtype == WTYPE_BITS && hdr.width == 32 && hdr.size < 500) {
        return collect_cluster_column_new(data, col_ref, col_type);
    }

    let partitions = read_array(data, col_ref).unwrap_or_default();
    let mut result = Vec::with_capacity(partitions.len());

    for &part_ref in &partitions {
        let pr = part_ref as usize;
        if pr == 0 || !pr.is_multiple_of(8) || pr + NODE_HEADER_SIZE > data.len() {
            result.push(Value::Null);
            continue;
        }
        let sub_refs = read_array(data, pr).unwrap_or_default();

        // Detect compact string layout (offsets + blob + bitmap) vs generic follow-refs
        let mut items = Vec::new();
        if sub_refs.len() >= 2 {
            let a = sub_refs[0] as usize;
            let b = sub_refs[1] as usize;
            if a != 0
                && a.is_multiple_of(8)
                && b != 0
                && b.is_multiple_of(8)
                && a + NODE_HEADER_SIZE <= data.len()
                && b + NODE_HEADER_SIZE <= data.len()
            {
                let ah = NodeHeader::parse(data[a..a + 8].try_into().unwrap());
                let bh = NodeHeader::parse(data[b..b + 8].try_into().unwrap());
                if bh.wtype == WTYPE_IGNORE
                    && (ah.width == 16 || ah.width == 8)
                    && ah.wtype == WTYPE_BITS
                {
                    let compact_vals = read_partitioned_compact_string(data, &sub_refs);
                    for v in compact_vals {
                        if let Value::String(s) = v {
                            items.push(s);
                        }
                    }
                }
            }
        }

        if items.is_empty() {
            for &sr in &sub_refs {
                let sref = sr as usize;
                if sref == 0 || !sref.is_multiple_of(8) || sref + NODE_HEADER_SIZE > data.len() {
                    continue;
                }
                let shdr = NodeHeader::parse(data[sref..sref + 8].try_into().unwrap());
                if shdr.is_inner || shdr.wtype == WTYPE_LINKLIST {
                    continue;
                }
                if shdr.wtype == WTYPE_IGNORE {
                    // Realm files can contain inline binary blobs (3-10MB png/wav).
                    // Real string values never exceed a few KB; cap at 64KB and read
                    // only the first null-terminated segment.
                    if shdr.size > 65536 {
                        continue;
                    }
                    let blob = &data[sref + NODE_HEADER_SIZE..];
                    let len = shdr.size.min(blob.len());
                    if let Some(null_pos) = blob[..len].iter().position(|&b| b == 0) {
                        if null_pos > 0 {
                            let s = String::from_utf8_lossy(&blob[..null_pos]);
                            if !s.is_empty() {
                                items.push(s.into_owned());
                            }
                        }
                    } else if len > 0 {
                        let s = String::from_utf8_lossy(&blob[..len]);
                        if !s.is_empty() {
                            items.push(s.into_owned());
                        }
                    }
                } else {
                    let vals = collect_leaf_values_new(data, sref, &shdr, col_type);
                    for v in vals {
                        match v {
                            Value::String(s) if !s.is_empty() => items.push(s),
                            Value::Null => {}
                            other => items.push(format!("{other:?}")),
                        }
                    }
                }
            }
        }
        result.push(if items.is_empty() {
            Value::Null
        } else {
            Value::String(items.join("\n"))
        });
    }
    result
}

/// Map a column index to its position in the cluster root array.
///
/// col\[0\] → cluster\[0\]; pk_index at cluster\[1\] is skipped.
/// For col\[k ≥ 1\]: start at cluster\[2\] and account for multi-slot types
/// encountered in cols 1..k-1.
///
/// Multi-slot types (realm-core native codes):
///   8  = Timestamp (2 slots: seconds + nanoseconds)
///   17 = UUID      (2 slots: hi64 + lo64)
///   14 = BackLink  (0 slots: virtual column)
fn cluster_index_for_col(col_idx: usize, col_type_ints: &[u64]) -> usize {
    if col_idx == 0 {
        return 0;
    }
    // cluster[0] = pk_col, cluster[1] = pk_index (B+ tree, skipped).
    // Data columns start at cluster[2].
    let mut ci = 2usize;
    for k in 1..col_idx {
        let ct = col_type_ints.get(k).copied().unwrap_or(0) as u8;
        match ct {
            8 => ci += 2,  // Timestamp: seconds + nanoseconds = 2 cluster slots
            14 => {}       // BackLink: virtual column, 0 cluster slots
            17 => ci += 2, // UUID: 2 cluster slots
            _ => ci += 1,
        }
    }
    ci
}

// ── New-format string collection ──────────────────────────────────────────────

/// Collect all strings from a column's B+ tree (new format).
///
/// Inner nodes: `[offsets_tracking_ref, child_0, ..., child_{N-1}]`
/// - element 0 is always the offsets-tracking node (skip).
/// - garbage refs (misaligned addresses) are detected and skipped.
///
/// Collect column values from a v24 cluster-tree column.
///
/// Data columns in v24 cluster trees use leaf-of-refs partitioning: the root
/// entry is a ``wtype=0`` leaf whose 32-bit elements point to per-row value
/// nodes. This function follows that chain.
fn collect_cluster_column_new(data: &[u8], col_ref: usize, col_type: ColumnType) -> Vec<Value> {
    let val_cap = 500_000;
    collect_cluster_column_new_inner(data, col_ref, col_type, 0, &mut 0u32, val_cap)
}

fn collect_cluster_column_new_inner(
    data: &[u8],
    col_ref: usize,
    col_type: ColumnType,
    depth: u32,
    total_out: &mut u32,
    cap: u32,
) -> Vec<Value> {
    if *total_out >= cap || depth > 30 {
        return vec![];
    }
    if col_ref == 0 || !col_ref.is_multiple_of(8) || col_ref + NODE_HEADER_SIZE > data.len() {
        return vec![];
    }
    let hdr = NodeHeader::parse(data[col_ref..col_ref + 8].try_into().unwrap());

    if !hdr.is_inner && hdr.wtype == WTYPE_BITS && hdr.width == 32 && hdr.size < 1000 {
        let refs = match read_array(data, col_ref) {
            Ok(r) => r,
            Err(_) => return vec![],
        };

        // 32-bit leaf: could be file-offset refs or inline data (e.g. f32).
        // If the first few elements are not 8-byte aligned, treat as raw data.
        let looks_like_refs =
            refs.len() >= 2 && refs.iter().take(3).all(|&r| r == 0 || r.is_multiple_of(8));
        if !looks_like_refs {
            return collect_leaf_values_new(data, col_ref, &hdr, col_type);
        }

        // Partitioned leaf-of-refs: detect compact-string offsets+blob layout.
        if refs.len() >= 2 {
            let a = refs[0] as usize;
            let b = refs[1] as usize;
            if a != 0
                && a.is_multiple_of(8)
                && b != 0
                && b.is_multiple_of(8)
                && a + 8 <= data.len()
                && b + 8 <= data.len()
            {
                let ah = NodeHeader::parse(data[a..a + 8].try_into().unwrap());
                let bh = NodeHeader::parse(data[b..b + 8].try_into().unwrap());
                if bh.wtype == WTYPE_IGNORE
                    && (ah.width == 16 || ah.width == 8)
                    && ah.wtype == WTYPE_BITS
                {
                    return read_partitioned_compact_string(data, &refs);
                }
            }
        }

        // Generic leaf-of-refs: follow each ref as a leaf holding row-block values.
        let mut values: Vec<Value> = Vec::new();
        for elem in refs {
            if values.len() >= 200_000 {
                break;
            }
            let row_node_ref = elem as usize;
            if row_node_ref == 0
                || !row_node_ref.is_multiple_of(8)
                || row_node_ref + NODE_HEADER_SIZE > data.len()
            {
                continue;
            }
            let row_hdr =
                NodeHeader::parse(data[row_node_ref..row_node_ref + 8].try_into().unwrap());
            if row_hdr.is_inner {
                let sub = collect_cluster_column_new_inner(
                    data,
                    row_node_ref,
                    col_type,
                    depth + 1,
                    total_out,
                    cap,
                );
                values.extend(sub);
            } else {
                let block_vals = collect_leaf_values_new(data, row_node_ref, &row_hdr, col_type);
                let n = block_vals.len();
                values.extend(block_vals);
                *total_out += n as u32;
            }
        }
        return values;
    }

    if hdr.is_inner {
        collect_ints_new(data, col_ref)
            .into_iter()
            .map(|v| value_from_int(v, col_type))
            .collect()
    } else {
        collect_leaf_values_new(data, col_ref, &hdr, col_type)
    }
}

/// Read a compact-string layout spread across multiple leaf-of-refs slots:
/// slot\[0\] = offsets array (16-bit or 8-bit ints)
/// slot\[1\] = blob (wtype=2 null-separated strings)
/// slot\[2\] = null bitmap (wtype=0, 1-bit), optional
fn read_partitioned_compact_string(data: &[u8], refs: &[u64]) -> Vec<Value> {
    let offsets_ref = refs[0] as usize;
    let blob_ref = refs[1] as usize;

    let offsets = read_array(data, offsets_ref).unwrap_or_default();

    let blob_hdr = NodeHeader::parse(data[blob_ref..blob_ref + 8].try_into().unwrap());
    let blob_end = (blob_ref + NODE_HEADER_SIZE + blob_hdr.size).min(data.len());
    let blob = &data[blob_ref + NODE_HEADER_SIZE..blob_end];

    let mut values = Vec::with_capacity(offsets.len());
    for r in 0..offsets.len() {
        let start = if r == 0 { 0 } else { offsets[r - 1] as usize };
        let raw_end = offsets[r] as usize;
        let end = if raw_end > 0 {
            (raw_end - 1).min(blob.len())
        } else {
            start
        };
        let start = start.min(end);
        values.push(Value::String(
            String::from_utf8_lossy(&blob[start..end]).into_owned(),
        ));
    }
    values
}

fn collect_leaf_values_new(
    data: &[u8],
    leaf_ref: usize,
    hdr: &NodeHeader,
    col_type: ColumnType,
) -> Vec<Value> {
    match col_type {
        ColumnType::String | ColumnType::Data => collect_strings_new(data, leaf_ref)
            .into_iter()
            .map(Value::String)
            .collect(),
        ColumnType::Bool => collect_ints_new(data, leaf_ref)
            .into_iter()
            .map(|v| Value::Bool(v != 0))
            .collect(),
        ColumnType::Float if hdr.wtype == WTYPE_BITS && hdr.width == 32 => {
            let ints = collect_ints_new(data, leaf_ref);
            ints.into_iter()
                .map(|v| Value::Float(f32::from_bits(v as u32) as f64))
                .collect()
        }
        ColumnType::Float if hdr.wtype == WTYPE_MULTIPLY && hdr.width == 4 => {
            read_multiply_f32_leaf(data, leaf_ref, hdr)
        }
        ColumnType::Double if hdr.wtype == WTYPE_MULTIPLY && hdr.width == 8 => {
            read_multiply_f64_leaf(data, leaf_ref, hdr)
        }
        ColumnType::Float | ColumnType::Double => {
            let ints = collect_ints_new(data, leaf_ref);
            ints.into_iter()
                .map(|v| Value::Float(f64::from_bits(v)))
                .collect()
        }
        ColumnType::Int => collect_ints_new(data, leaf_ref)
            .into_iter()
            .map(|v| Value::Int(v as i64))
            .collect(),
        ColumnType::Timestamp => collect_ints_new(data, leaf_ref)
            .into_iter()
            .map(|v| Value::Timestamp(v as i64))
            .collect(),
        _ => {
            let ints = collect_ints_new(data, leaf_ref);
            ints.into_iter().map(|v| Value::Int(v as i64)).collect()
        }
    }
}

fn read_multiply_f32_leaf(data: &[u8], leaf_ref: usize, hdr: &NodeHeader) -> Vec<Value> {
    let payload_start = leaf_ref + NODE_HEADER_SIZE;
    let needed = hdr.size * hdr.width as usize;
    if payload_start + needed > data.len() {
        return (0..hdr.size).map(|_| Value::Null).collect();
    }
    let payload = &data[payload_start..];
    (0..hdr.size)
        .map(|i| {
            let off = i * 4;
            let bytes: [u8; 4] = payload[off..off + 4].try_into().unwrap_or([0; 4]);
            let f = f32::from_le_bytes(bytes);
            Value::Float(f as f64)
        })
        .collect()
}

fn read_multiply_f64_leaf(data: &[u8], leaf_ref: usize, hdr: &NodeHeader) -> Vec<Value> {
    let payload_start = leaf_ref + NODE_HEADER_SIZE;
    let needed = hdr.size * hdr.width as usize;
    if payload_start + needed > data.len() {
        return (0..hdr.size).map(|_| Value::Null).collect();
    }
    let payload = &data[payload_start..];
    (0..hdr.size)
        .map(|i| {
            let off = i * 8;
            let bytes: [u8; 8] = payload[off..off + 8].try_into().unwrap_or([0; 8]);
            let f = f64::from_le_bytes(bytes);
            Value::Float(f)
        })
        .collect()
}

fn value_from_int(v: u64, col_type: ColumnType) -> Value {
    match col_type {
        ColumnType::Bool => Value::Bool(v != 0),
        ColumnType::Int => Value::Int(v as i64),
        ColumnType::Timestamp => Value::Timestamp(v as i64),
        ColumnType::Float | ColumnType::Double => Value::Float(f64::from_bits(v)),
        _ => Value::Int(v as i64),
    }
}

fn collect_strings_new(data: &[u8], col_ref: usize) -> Vec<String> {
    if col_ref == 0 || !col_ref.is_multiple_of(8) || col_ref + NODE_HEADER_SIZE > data.len() {
        return vec![];
    }
    let mut result = vec![];
    let mut queue = vec![col_ref];
    while let Some(ref_addr) = queue.pop() {
        if ref_addr == 0 || !ref_addr.is_multiple_of(8) || ref_addr + NODE_HEADER_SIZE > data.len()
        {
            continue;
        }
        let hdr = NodeHeader::parse(data[ref_addr..ref_addr + 8].try_into().unwrap());
        if hdr.is_inner {
            let children = match read_array(data, ref_addr) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (j, &child_u64) in children.iter().enumerate() {
                if j == 0 {
                    continue;
                }
                let child_ref = child_u64 as usize;
                if child_ref != 0
                    && child_ref.is_multiple_of(8)
                    && child_ref + NODE_HEADER_SIZE <= data.len()
                {
                    queue.push(child_ref);
                }
            }
        } else {
            result.extend(collect_string_leaf(data, ref_addr, &hdr));
        }
    }
    result
}

/// Dispatch to the correct leaf reader based on node shape.
///
/// Three leaf types observed in Realm 5+ string columns:
///   1. Compact string  - `wtype=0, size ∈ {2,3}`: `[offsets_ref, blob_ref]`
///   2. Per-row refs    - `wtype=0, width ≥ 16, size > 3`: each elem → wtype=2 node
///   3. Inline strings  - `wtype=1`: fixed-width slots, `decode_short_string` encoding
fn collect_string_leaf(data: &[u8], leaf_ref: usize, hdr: &NodeHeader) -> Vec<String> {
    match hdr.wtype {
        WTYPE_BITS => {
            if hdr.size <= 3 {
                read_compact_string_leaf(data, leaf_ref).unwrap_or_default()
            } else if hdr.size < 100_000 && hdr.width >= 16 {
                read_perrow_string_refs(data, leaf_ref)
            } else {
                vec![]
            }
        }
        WTYPE_MULTIPLY if hdr.width > 0 && hdr.size > 0 && hdr.size < 100_000 => {
            let payload_start = leaf_ref + NODE_HEADER_SIZE;
            let needed = hdr.size * hdr.width as usize;
            if payload_start + needed > data.len() {
                return vec![];
            }
            let payload = &data[payload_start..];
            (0..hdr.size)
                .map(|i| decode_short_string(multiply_elem_bytes(payload, i, hdr.width)))
                .collect()
        }
        _ => vec![],
    }
}

/// Read a compact-string leaf node: `[offsets_ref, blob_ref]`.
///
/// `offsets[r]` = byte offset one past the null terminator of string r.
/// The blob is a wtype=2 (WTYPE_IGNORE) node holding the concatenated
/// null-terminated UTF-8 strings.
fn read_compact_string_leaf(data: &[u8], leaf_ref: usize) -> Result<Vec<String>> {
    let elems = read_array(data, leaf_ref)?;
    if elems.len() < 2 {
        return Ok(vec![]);
    }
    let offsets_ref = elems[0] as usize;
    let blob_ref = elems[1] as usize;

    if offsets_ref == 0 || offsets_ref + NODE_HEADER_SIZE > data.len() {
        return Ok(vec![]);
    }
    if blob_ref == 0 || blob_ref + NODE_HEADER_SIZE > data.len() {
        return Ok(vec![]);
    }

    let blob_hdr = NodeHeader::parse(data[blob_ref..blob_ref + 8].try_into().unwrap());
    if blob_hdr.wtype != WTYPE_IGNORE {
        return Ok(vec![]);
    }
    let blob_end = (blob_ref + NODE_HEADER_SIZE + blob_hdr.size).min(data.len());
    let blob = &data[blob_ref + NODE_HEADER_SIZE..blob_end];

    let offsets = read_array(data, offsets_ref)?;
    let mut strings = Vec::with_capacity(offsets.len());
    for r in 0..offsets.len() {
        let start = if r == 0 { 0 } else { offsets[r - 1] as usize };
        // offsets[r] points one past the null terminator; subtract 1 for the string end
        let raw_end = offsets[r] as usize;
        let end = if raw_end > 0 {
            (raw_end - 1).min(blob.len())
        } else {
            start
        };
        let start = start.min(end);
        strings.push(String::from_utf8_lossy(&blob[start..end]).into_owned());
    }
    Ok(strings)
}

/// Read a per-row string ref leaf: each element is a file offset to a wtype=2 node.
fn read_perrow_string_refs(data: &[u8], leaf_ref: usize) -> Vec<String> {
    let refs = match read_array(data, leaf_ref) {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    refs.into_iter()
        .map(|str_ref_u64| {
            let str_ref = str_ref_u64 as usize;
            if str_ref == 0 || !str_ref.is_multiple_of(8) || str_ref + NODE_HEADER_SIZE > data.len()
            {
                return String::new();
            }
            read_wtype2_string(data, str_ref).unwrap_or_default()
        })
        .collect()
}

/// Read raw UTF-8 from a wtype=2 (WTYPE_IGNORE) node, stripping a trailing null if present.
fn read_wtype2_string(data: &[u8], str_ref: usize) -> Result<String> {
    let hdr = NodeHeader::parse(data[str_ref..str_ref + 8].try_into().unwrap());
    if hdr.wtype != WTYPE_IGNORE {
        return Ok(String::new());
    }
    let payload = &data[str_ref + NODE_HEADER_SIZE..];
    let len = hdr.size.min(payload.len());
    let end = if len > 0 && payload[len - 1] == 0 {
        len - 1
    } else {
        len
    };
    Ok(String::from_utf8_lossy(&payload[..end]).into_owned())
}

// ── New-format integer collection ─────────────────────────────────────────────

/// Collect all integer elements from a column's B+ tree (new format).
///
/// Same inner-node layout as for strings: skip element 0 (offsets-tracking),
/// filter garbage children by alignment.
fn collect_ints_new(data: &[u8], col_ref: usize) -> Vec<u64> {
    if col_ref == 0 || !col_ref.is_multiple_of(8) || col_ref + NODE_HEADER_SIZE > data.len() {
        return vec![];
    }
    let mut result = Vec::new();
    let mut queue = vec![col_ref];
    while let Some(ref_addr) = queue.pop() {
        if result.len() >= 50_000 {
            break;
        }
        if ref_addr == 0 || !ref_addr.is_multiple_of(8) || ref_addr + NODE_HEADER_SIZE > data.len()
        {
            continue;
        }
        let hdr = NodeHeader::parse(data[ref_addr..ref_addr + 8].try_into().unwrap());
        if hdr.is_inner {
            let children = match read_array(data, ref_addr) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for (j, &child_u64) in children.iter().enumerate() {
                if j == 0 {
                    continue;
                }
                let child_ref = child_u64 as usize;
                if child_ref != 0
                    && child_ref.is_multiple_of(8)
                    && child_ref + NODE_HEADER_SIZE <= data.len()
                {
                    queue.push(child_ref);
                }
            }
        } else if hdr.wtype == WTYPE_BITS && hdr.size < 100_000 {
            let max = 50_000_u64.min(hdr.size as u64) as usize;
            let mut arr = read_array(data, ref_addr).unwrap_or_default();
            arr.truncate(max);
            result.extend(arr);
        } else if hdr.wtype == WTYPE_MULTIPLY && hdr.width > 0 && hdr.size < 100_000 {
            let payload_start = ref_addr + NODE_HEADER_SIZE;
            let needed = hdr.size * hdr.width as usize;
            if payload_start + needed > data.len() {
                continue;
            }
            let payload = &data[payload_start..];
            for i in 0..hdr.size {
                let slot = multiply_elem_bytes(payload, i, hdr.width);
                let val = match hdr.width {
                    8 => u64::from_le_bytes(slot.try_into().unwrap_or([0u8; 8])),
                    4 => u32::from_le_bytes(slot.try_into().unwrap_or([0u8; 4])) as u64,
                    2 => u16::from_le_bytes(slot.try_into().unwrap_or([0u8; 2])) as u64,
                    1 => slot[0] as u64,
                    _ => 0,
                };
                result.push(val);
            }
        }
    }
    result
}

// ── Old-format: row count + cell reading ──────────────────────────────────────

/// Count rows in a column node, traversing B-tree inner nodes if needed.
fn count_node_rows(data: &[u8], node_ref: usize) -> usize {
    if node_ref + NODE_HEADER_SIZE > data.len() {
        return 0;
    }
    let hdr = NodeHeader::parse(data[node_ref..node_ref + 8].try_into().unwrap());
    if !hdr.is_inner {
        return hdr.size;
    }
    if hdr.size == 0 {
        return 0;
    }
    // Old format inner node: last element is a ref to the cumulative-sizes array.
    let payload = &data[node_ref + NODE_HEADER_SIZE..];
    let sizes_ref = read_bits_elem(payload, hdr.size - 1, 64) as usize;
    read_array(data, sizes_ref)
        .ok()
        .and_then(|s| s.last().copied())
        .unwrap_or(0) as usize
}

fn read_cell(data: &[u8], col_ref: usize, row_idx: usize, col_type: ColumnType) -> Result<Value> {
    if col_ref + NODE_HEADER_SIZE > data.len() {
        return Ok(Value::Null);
    }
    let hdr = NodeHeader::parse(data[col_ref..col_ref + 8].try_into().unwrap());

    if hdr.is_inner {
        return read_cell_btree(data, col_ref, row_idx, col_type);
    }

    if row_idx >= hdr.size {
        return Ok(Value::Null);
    }

    let payload = &data[col_ref + NODE_HEADER_SIZE..];

    match col_type {
        ColumnType::Bool => Ok(Value::Bool(
            read_bits_elem(payload, row_idx, hdr.width) != 0,
        )),
        ColumnType::Int => Ok(Value::Int(
            read_bits_elem(payload, row_idx, hdr.width) as i64
        )),
        ColumnType::Timestamp => Ok(Value::Timestamp(
            read_bits_elem(payload, row_idx, hdr.width) as i64,
        )),
        ColumnType::Link | ColumnType::LinkList | ColumnType::BackLink => Ok(Value::Link(
            read_bits_elem(payload, row_idx, hdr.width) as usize,
        )),
        ColumnType::Float if hdr.width == 32 => {
            let off = row_idx * 4;
            let f = f32::from_le_bytes(payload[off..off + 4].try_into().unwrap_or([0; 4]));
            Ok(Value::Float(f as f64))
        }
        ColumnType::Double if hdr.width == 64 => {
            let off = row_idx * 8;
            let f = f64::from_le_bytes(payload[off..off + 8].try_into().unwrap_or([0; 8]));
            Ok(Value::Float(f))
        }
        ColumnType::String | ColumnType::Data => {
            if hdr.wtype == WTYPE_MULTIPLY && hdr.width > 0 {
                let slot = multiply_elem_bytes(payload, row_idx, hdr.width);
                Ok(Value::String(decode_short_string(slot)))
            } else if hdr.wtype == WTYPE_BITS && hdr.width == 64 {
                let str_ref = read_bits_elem(payload, row_idx, 64) as usize;
                if str_ref == 0 {
                    return Ok(Value::String(String::new()));
                }
                Ok(Value::String(read_leaf_string(data, str_ref)?))
            } else {
                Ok(Value::Null)
            }
        }
        _ => Ok(Value::Null),
    }
}

/// Traverse an old-format B-tree inner node to locate the leaf cell at `row_idx`.
///
/// Old-format layout: `[child_ref_0, ..., child_ref_n, cumulative_sizes_ref]`
/// `cumulative_sizes[i]` = total rows in subtrees 0..=i.
fn read_cell_btree(
    data: &[u8],
    node_ref: usize,
    row_idx: usize,
    col_type: ColumnType,
) -> Result<Value> {
    if node_ref + NODE_HEADER_SIZE > data.len() {
        return Ok(Value::Null);
    }
    let hdr = NodeHeader::parse(data[node_ref..node_ref + 8].try_into().unwrap());

    if !hdr.is_inner {
        return read_cell(data, node_ref, row_idx, col_type);
    }

    if hdr.size == 0 {
        return Ok(Value::Null);
    }

    let payload = &data[node_ref + NODE_HEADER_SIZE..];
    let n_children = hdr.size - 1;
    let sizes_ref = read_bits_elem(payload, hdr.size - 1, 64) as usize;
    let sizes = read_array(data, sizes_ref)?;

    let mut prev_cum = 0usize;
    for ci in 0..n_children {
        let cum = sizes.get(ci).copied().unwrap_or(0) as usize;
        if row_idx < cum {
            let child_ref = read_bits_elem(payload, ci, 64) as usize;
            return read_cell_btree(data, child_ref, row_idx - prev_cum, col_type);
        }
        prev_cum = cum;
    }

    Ok(Value::Null)
}

fn read_leaf_string(data: &[u8], str_ref: usize) -> Result<String> {
    if str_ref + NODE_HEADER_SIZE > data.len() {
        return Ok(String::new());
    }
    let hdr = NodeHeader::parse(data[str_ref..str_ref + 8].try_into().unwrap());
    let payload = &data[str_ref + NODE_HEADER_SIZE..];

    if hdr.wtype == WTYPE_MULTIPLY && hdr.width > 0 {
        let slot = multiply_elem_bytes(payload, 0, hdr.width);
        return Ok(decode_short_string(slot));
    }

    // Fallback: raw bytes up to first null byte
    let len = payload.len().min(hdr.size);
    let end = payload[..len].iter().position(|&b| b == 0).unwrap_or(len);
    Ok(String::from_utf8_lossy(&payload[..end]).into_owned())
}
