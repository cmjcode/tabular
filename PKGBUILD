pkgname=tabular
pkgver=1.0.0
pkgrel=1
arch=('x86_64')
url="https://github.com/yourname/tabular"
license=('MIT')
depends=()
makedepends=('rust' 'cargo')
source=("$url/archive/v$pkgver.tar.gz")
sha256sums=('SKIP')

build() {
    cd "$pkgname-$pkgver"
    cargo build --release
}

package() {
    cd "$pkgname-$pkgver"
    install -Dm755 target/release/tabular "$pkgdir/usr/bin/tabular"
}
