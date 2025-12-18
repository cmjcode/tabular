# Flatpak Build & Publish Guide

## Build Lokal

### 1. Generate cargo dependencies (sekali atau tiap update deps):
```bash
sh flatpak/generate-cargo-sources.sh
```

### 2. Build Flatpak lokal:
```bash
sh flatpak_build.sh
```

### 3. Test install lokal:
```bash
flatpak --user remote-add --if-not-exists tabular-local file://$(pwd)/flatpak/repo
flatpak --user install tabular-local id.tabular.database
flatpak run id.tabular.database
```

### 4. Buat bundle distribusi:
```bash
sh flatpak_publish.sh
```

Bundle akan ada di: `flatpak/dist/id.tabular.database-stable.flatpak`

---

## Submit ke Flathub

### Persiapan

1. **Generate cargo sources untuk Flathub:**
```bash
cd flatpak
python3 flatpak-cargo-generator.py ../Cargo.lock -o generated-sources.json
```

2. **Update commit hash di manifest:**
   
   Setelah push tag release ke GitHub:
   ```bash
   git tag v0.5.27
   git push origin v0.5.27
   ```
   
   Ambil commit hash:
   ```bash
   git rev-parse v0.5.27
   ```
   
   Update `commit: COMMIT_HASH_HERE` di `flatpak/id.tabular.database.flathub.yml`

3. **Upload screenshot:**
   - Upload screenshot aplikasi ke folder `screenshots/` di repo GitHub
   - Pastikan URL di `id.tabular.database.metainfo.xml` valid

### Submit PR ke Flathub

1. **Fork flathub/flathub:**
   ```bash
   # Di GitHub: https://github.com/flathub/flathub -> Fork
   ```

2. **Clone & setup:**
   ```bash
   git clone https://github.com/YOUR_USERNAME/flathub
   cd flathub
   git checkout -b add-tabular
   ```

3. **Copy files:**
   ```bash
   # Buat folder app
   mkdir -p id.tabular.database
   
   # Copy manifest
   cp /path/to/tabular/flatpak/id.tabular.database.flathub.yml \
      id.tabular.database/id.tabular.database.yml
   
   # Copy generated sources
   cp /path/to/tabular/flatpak/generated-sources.json \
      id.tabular.database/generated-sources.json
   ```

4. **Commit & push:**
   ```bash
   git add id.tabular.database/
   git commit -m "Add Tabular database client"
   git push origin add-tabular
   ```

5. **Buat Pull Request:**
   - Buka https://github.com/flathub/flathub
   - Create Pull Request dari branch `add-tabular`
   - Isi deskripsi:
     - Link ke repo: https://github.com/tabular-id/tabular
     - Link ke license
     - Screenshot
     - Penjelasan singkat aplikasi

### Review Process

Flathub akan:
- ✅ Test build manifest
- ✅ Check license compliance
- ✅ Validate metadata (AppData)
- ✅ Check icon & screenshot
- ✅ Review permissions (finish-args)

Jika ada masalah, maintainer akan kasih feedback di PR.

### Update Release

Untuk update ke versi baru:

1. Update `Cargo.toml` version
2. Generate ulang `generated-sources.json`
3. Update tag & commit di manifest
4. Update `<releases>` di metainfo.xml
5. PR update ke flathub/id.tabular.database repo (bukan flathub/flathub)

---

## File Penting

- `flatpak/id.tabular.database.yml` - Manifest lokal (pakai cargo-vendor)
- `flatpak/id.tabular.database.flathub.yml` - Manifest Flathub (pakai generated-sources)
- `id.tabular.database.metainfo.xml` - AppStream metadata (wajib)
- `assets/logo-512.png` - Icon 512x512 (max size untuk Flatpak)
- `flatpak/generated-sources.json` - Cargo dependencies untuk Flathub

## Troubleshooting

**Icon terlalu besar:**
```bash
# Resize dengan ImageMagick
magick assets/logo.png -resize 512x512 assets/logo-512.png
```

**Missing libclang:**
- Sudah include `org.freedesktop.Sdk.Extension.llvm18` di manifest

**Network error saat build:**
- Pakai `cargo vendor` atau `generated-sources.json` untuk offline build
