name := 'cosmic-applet-google-agenda'
appid := 'com.github.ragusa87.CosmicAppletGoogleAgenda'

rootdir := ''
prefix := '/usr'

base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
bin-dst := base-dir / 'bin' / name
desktop-dst := base-dir / 'share' / 'applications' / appid + '.desktop'
icon-dst := base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg'

home := env('HOME')
user-base-dir := home / '.local'
user-bin-dst := user-base-dir / 'bin' / name
user-desktop-dst := user-base-dir / 'share' / 'applications' / appid + '.desktop'
user-icon-dst := user-base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg'

default: build-release

clean:
    cargo clean

build-debug *args:
    cargo build {{args}}

build-release *args: (build-debug '--release' args)

check *args:
    cargo clippy --all-features {{args}} -- -W clippy::pedantic

run *args:
    env RUST_BACKTRACE=full cargo run --release {{args}}

install:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{bin-dst}}
    install -Dm0644 data/{{appid}}.desktop {{desktop-dst}}
    install -Dm0644 data/icons/{{appid}}.svg {{icon-dst}}

install-user:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{user-bin-dst}}
    install -Dm0644 data/{{appid}}.desktop {{user-desktop-dst}}
    install -Dm0644 data/icons/{{appid}}.svg {{user-icon-dst}}

uninstall:
    rm -f {{bin-dst}} {{desktop-dst}} {{icon-dst}}

uninstall-user:
    rm -f {{user-bin-dst}} {{user-desktop-dst}} {{user-icon-dst}}
