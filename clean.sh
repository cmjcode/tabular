#!/usr/bin/env bash
# ==============================================================================
# Script: clean.sh
# Tujuan: Menghapus kontributor Claude / Anthropic dari riwayat Git
#         (membersihkan Co-Authored-By, Author, dan Committer)
# ==============================================================================

set -euo pipefail

# Definisi warna output terminal
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

echo -e "${BLUE}${BOLD}=====================================================${NC}"
echo -e "${BLUE}${BOLD}   Pembersih Kontributor Claude dari Riwayat Git     ${NC}"
echo -e "${BLUE}${BOLD}=====================================================${NC}\n"

# 1. Validasi dependensi
if ! command -v git &>/dev/null; then
  echo -e "${RED}Error: 'git' tidak ditemukan di sistem.${NC}"
  exit 1
fi

if ! command -v git-filter-repo &>/dev/null; then
  echo -e "${RED}Error: 'git-filter-repo' belum terpasang.${NC}"
  echo -e "Silakan install terlebih dahulu dengan perintah:"
  echo -e "  ${CYAN}brew install git-filter-repo${NC}\n"
  exit 1
fi

if ! git rev-parse --is-inside-work-tree &>/dev/null; then
  echo -e "${RED}Error: Direktori ini bukan repositori Git.${NC}"
  exit 1
fi

# Fungsi hitung commit yang terkait Claude / Anthropic
count_claude_commits() {
  (
    git log --all -i -E --grep="co-authored-by:.*(claude|anthropic)" --format="%h" 2>/dev/null || true
    git log --all -i -E --author="claude|anthropic" --format="%h" 2>/dev/null || true
    git log --all -i -E --committer="claude|anthropic" --format="%h" 2>/dev/null || true
  ) | sed '/^$/d' | sort -u | wc -l | tr -d ' '
}

# 2. Hitung jumlah commit yang memiliki metadata Claude
MATCHING_COMMITS=$(count_claude_commits)
echo -e "Ditemukan ${YELLOW}${BOLD}${MATCHING_COMMITS}${NC} commit yang memiliki metadata Claude/Anthropic."

if [ "$MATCHING_COMMITS" -eq 0 ]; then
  echo -e "${GREEN}${BOLD}Repositori sudah bersih! Tidak ada jejak Claude/Anthropic di riwayat Git.${NC}"
  exit 0
fi

# 3. Konfirmasi dari pengguna
AUTO_CONFIRM=false
for arg in "$@"; do
  if [ "$arg" == "-y" ] || [ "$arg" == "--yes" ]; then
    AUTO_CONFIRM=true
  fi
done

if [ "$AUTO_CONFIRM" = false ] && [ -t 0 ]; then
  echo -e "\n${YELLOW}${BOLD}Perhatian:${NC}"
  echo -e "Proses ini akan menulis ulang riwayat Git (history rewrite) untuk semua branch."
  echo -e "Script akan membuat ${BOLD}backup bundle otomatis${NC} sebelum memodifikasi riwayat."
  read -r -p "Lanjutkan pembersihan riwayat Git? (y/N): " response
  case "$response" in
    [yY][eE][sS]|[yY]) ;;
    *) echo -e "\n${RED}Operasi dibatalkan oleh pengguna.${NC}"; exit 0 ;;
  esac
fi

# 4. Buat backup bundle repositori
BACKUP_DIR="${HOME}/.git_backups"
mkdir -p "$BACKUP_DIR"
REPO_NAME=$(basename "$(git rev-parse --show-toplevel)")
TIMESTAMP=$(date +"%Y%m%d_%H%M%S")
BACKUP_FILE="${BACKUP_DIR}/${REPO_NAME}_backup_${TIMESTAMP}.bundle"

echo -e "\n${CYAN}Membuat backup lengkap riwayat repositori ke:${NC}"
echo -e "  ${BACKUP_FILE}"
git bundle create "$BACKUP_FILE" --all >/dev/null
echo -e "${GREEN}✓ Backup berhasil dibuat.${NC}"

# 5. Simpan URL remote origin (karena git-filter-repo otomatis menghapus remote origin)
ORIGIN_URL=$(git remote get-url origin 2>/dev/null || true)
if [ -n "$ORIGIN_URL" ]; then
  echo -e "Remote origin saat ini: ${CYAN}${ORIGIN_URL}${NC}"
fi

# 6. Simpan perubahan uncommitted sementara bila ada
STASHED=0
if [ -n "$(git status --porcelain)" ]; then
  echo -e "${YELLOW}Menyimpan perubahan uncommitted ke stash...${NC}"
  git stash push -u -m "clean-claude-temp-stash" >/dev/null
  STASHED=1
fi

# 7. Siapkan callback Python untuk git-filter-repo
CALLBACK_FILE=$(mktemp /tmp/clean_claude_cb.XXXXXX.py)
trap 'rm -f "$CALLBACK_FILE"' EXIT

cat << 'EOF' > "$CALLBACK_FILE"
# 1. Ganti author jika menggunakan email atau nama Claude / Anthropic
if b"anthropic" in commit.author_email.lower() or b"claude" in commit.author_name.lower():
    commit.author_name = b"YNP. Jayuda"
    commit.author_email = b"yulius.Jayuda@gmail.com"

# 2. Ganti committer jika menggunakan email atau nama Claude / Anthropic
if b"anthropic" in commit.committer_email.lower() or b"claude" in commit.committer_name.lower():
    commit.committer_name = b"YNP. Jayuda"
    commit.committer_email = b"yulius.Jayuda@gmail.com"

# 3. Hapus baris 'Co-Authored-By' atau 'Signed-off-by' yang merujuk ke Claude / Anthropic
filtered_lines = []
for line in commit.message.splitlines():
    lower_line = line.lower().strip()
    is_trailer = lower_line.startswith(b"co-authored-by:") or lower_line.startswith(b"signed-off-by:")
    if is_trailer and (b"claude" in lower_line or b"anthropic" in lower_line):
        continue
    filtered_lines.append(line)

commit.message = b"\n".join(filtered_lines).rstrip() + b"\n"
EOF

# 8. Eksekusi git-filter-repo
echo -e "\n${CYAN}Menjalankan git-filter-repo...${NC}"
git-filter-repo --force --commit-callback "$CALLBACK_FILE"

# 9. Pulihkan remote origin
if [ -n "$ORIGIN_URL" ]; then
  echo -e "\n${CYAN}Mengembalikan remote origin...${NC}"
  git remote add origin "$ORIGIN_URL"
  echo -e "${GREEN}✓ Remote origin berhasil dipulihkan: ${ORIGIN_URL}${NC}"
fi

# 10. Kembalikan perubahan stash bila ada
if [ "$STASHED" -eq 1 ]; then
  echo -e "\n${CYAN}Mengembalikan perubahan uncommitted dari stash...${NC}"
  git stash pop >/dev/null 2>&1 || echo -e "${YELLOW}Catatan: Silakan jalankan 'git stash pop' manual jika terjadi conflict.${NC}"
fi

# 11. Verifikasi hasil pembersihan
REMAINING=$(count_claude_commits)

echo -e "\n${GREEN}${BOLD}=====================================================${NC}"
echo -e "${GREEN}${BOLD}              Pembersihan Selesai!                  ${NC}"
echo -e "${GREEN}${BOLD}=====================================================${NC}"
echo -e "Commit Claude tersisa di riwayat: ${BOLD}${REMAINING}${NC}"

# 12. Langkah selanjutnya
echo -e "\n${YELLOW}${BOLD}Langkah Selanjutnya (Push ke GitHub):${NC}"
echo -e "Riwayat commit lokal Anda sekarang sudah 100% bersih."
echo -e "Untuk memperbarui repository di GitHub agar Claude hilang dari daftar kontributor:"
echo -e "\n  ${CYAN}git push origin --force --all${NC}"
echo -e "  ${CYAN}git push origin --force --tags${NC}\n"
echo -e "${YELLOW}Catatan Tambahan:${NC}"
echo -e "1. Jika branch dilindungi (Branch Protection Rules) di GitHub, buka repository Settings -> Branches,"
echo -e "   dan centang 'Allow force pushes' atau nonaktifkan proteksi sementara sebelum push."
echo -e "2. GitHub memerlukan waktu beberapa menit untuk memperbarui tab Contributor setelah force push."
echo -e "3. File backup tersimpan dengan aman di:"
echo -e "   ${CYAN}${BACKUP_FILE}${NC}"
echo -e "   (Bisa digunakan untuk restore kapan saja jika diperlukan)."
