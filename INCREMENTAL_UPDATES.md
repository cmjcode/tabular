# Incremental Tree Updates

## Overview

Implementasi incremental updates untuk sidebar tree agar lebih responsif dan efisien. Sebelumnya, setiap perubahan (add, edit, delete connection/table) akan rebuild seluruh tree dan kehilangan expansion state.

## Changes Made

### Connection Operations

#### 1. **Add Connection** (`add_connection_to_tree`)
- ✅ Menambah connection baru langsung ke folder yang sesuai
- ✅ Maintain sort order otomatis
- ✅ Create folder baru jika belum ada
- ✅ Preserve expansion state folder lainnya

#### 2. **Edit Connection** (`update_connection_in_tree`)
- ✅ Update node yang spesifik saja
- ✅ Preserve expansion state dan children (databases/tables yang sudah di-load)
- ✅ Handle perpindahan antar folder
- ✅ Cleanup empty folders otomatis

#### 3. **Delete Connection** (`remove_connection_from_tree`)
- ✅ Hapus node yang spesifik saja
- ✅ Cleanup empty folders (kecuali "Default")
- ✅ No full tree rebuild

#### 4. **Copy Connection**
- ✅ Gunakan `add_connection_to_tree` untuk insert tanpa rebuild

### Table Operations

#### 1. **Drop Table** (`remove_table_from_tree`)
- ✅ Hapus table dari tree secara incremental
- ✅ Clear cache untuk table tersebut saja
- ✅ Preserve expansion state database lainnya
- ✅ No need to refresh entire connection

#### 2. **Create Table** (`add_table_to_tree`)
- ✅ Tambah table baru ke tree (ready for future use)
- ✅ Maintain sort order otomatis
- ✅ Preserve state tree lainnya

### Helper Functions

#### Expansion State Management
- `save_tree_expansion_states()` - Menyimpan state expand/collapse
- `restore_tree_expansion_states()` - Restore state setelah rebuild
- `refresh_connections_tree()` - Otomatis preserve state saat rebuild (fallback)

#### Cache Management
- `clear_table_cache()` - Clear cache untuk specific table
  - table_cache
  - column_cache
  - row_cache
  - index_cache

## Benefits

### Performance
- **Faster updates** - No full tree rebuild untuk setiap perubahan
- **Less memory** - Tidak allocate ulang seluruh tree structure
- **Better responsiveness** - UI tidak freeze saat update

### User Experience
- **State preserved** - Expansion state tidak hilang
- **Children preserved** - Databases/tables yang sudah di-load tidak hilang
- **No flicker** - UI lebih smooth tanpa rebuild flash
- **Context preserved** - User tidak kehilangan posisi saat edit

### Before vs After

**Before:**
```
Action → Clear entire tree → Rebuild from scratch → Lose all state ❌
```

**After:**
```
Add    → Find/create folder → Insert sorted → Done ✨
Edit   → Find node → Update in-place → Move if needed → Preserve children ✨
Delete → Find node → Remove → Cleanup empty → Done ✨
```

## Code Locations

### Main Files
- `src/sidebar_database.rs` - Connection tree operations
- `src/window_egui.rs` - Table tree operations & drop table dialog

### Key Functions

**sidebar_database.rs:**
- `add_connection_to_tree()`
- `update_connection_in_tree()`
- `remove_connection_from_tree()`
- `add_table_to_tree()`
- `remove_table_from_tree()` (wrapper)
- `save_tree_expansion_states()`
- `restore_tree_expansion_states()`

**window_egui.rs:**
- `remove_table_from_tree()` (implementation)
- `clear_table_cache()`

## Future Enhancements

- [ ] Incremental update for CREATE TABLE (call `add_table_to_tree` after execution)
- [ ] Incremental update for RENAME TABLE
- [ ] Incremental update for database operations (CREATE/DROP DATABASE)
- [ ] Incremental update for view operations
- [ ] Incremental update for stored procedure operations
- [ ] Real-time sync dengan database changes (watching)

## Testing

Tested scenarios:
- ✅ Add connection → Tree updated incrementally
- ✅ Edit connection (same folder) → Node updated, children preserved
- ✅ Edit connection (different folder) → Moved to new folder, state preserved
- ✅ Delete connection → Removed from tree, no rebuild
- ✅ Copy connection → New node added incrementally
- ✅ Drop table → Table removed from tree, cache cleared

### How to Test Drop Table

1. **Setup**: Create a test connection to any database
2. **Expand tree**: Connection → Database → Tables folder → Select a table
3. **Drop table**: Right-click table → "Drop Table" → Confirm
4. **Expected behavior**:
   - Table disappears from sidebar immediately (~100ms)
   - No full connection refresh
   - Other expanded databases/tables remain expanded
   - Success message appears
5. **Check logs**: Look for these messages in terminal:
   ```
   ✅ DROP TABLE succeeded for database.table
   🌲 Removing table from sidebar tree (incremental)...
   🧹 Clearing cache for table database.table
   ✅ Removed table 'table_name' from tree
   ```

### Debugging

If table doesn't disappear from sidebar:
1. Check terminal logs for tree structure debug output
2. Verify connection_id, database_name, and table_name match
3. Check if tree structure uses DatabasesFolder or direct Database node
4. Verify table_name format (may include schema prefix like `dbo.table`)

## Notes

- Expansion state adalah path-based (folder_name > child_name)
- Empty folders dihapus otomatis kecuali "Default" folder
- Sort order dijaga secara alphabetical
- Database type icons preserved: 🐬 MySQL, 🐘 PostgreSQL, 📄 SQLite, 🔴 Redis, 🧰 MsSQL, 🍃 MongoDB
