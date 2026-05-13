rootdir := ''
prefix := '/usr'

base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')

home := env('HOME')
user-base-dir := home / '.local'

gmail-name := 'cosmic-applet-gmail'
gmail-appid := 'com.github.ragusa87.CosmicAppletGmail'
agenda-name := 'cosmic-applet-google-agenda'
agenda-appid := 'com.github.ragusa87.CosmicAppletGoogleAgenda'

default: build-release

clean:
    cargo clean

build-debug *args:
    cargo build {{args}}

build-release *args: (build-debug '--release' args)

check *args:
    cargo clippy --workspace --all-features {{args}} -- -W clippy::pedantic

run-gmail *args:
    env RUST_BACKTRACE=full cargo run --release -p cosmic-applet-gmail {{args}}

run-agenda *args:
    env RUST_BACKTRACE=full cargo run --release -p cosmic-applet-google-agenda {{args}}

install: \
    (_install-system gmail-name gmail-appid) \
    (_install-system agenda-name agenda-appid)

install-user: \
    (_install-user gmail-name gmail-appid) \
    (_install-user agenda-name agenda-appid)

uninstall: \
    (_uninstall-system gmail-name gmail-appid) \
    (_uninstall-system agenda-name agenda-appid)

uninstall-user: \
    (_uninstall-user gmail-name gmail-appid) \
    (_uninstall-user agenda-name agenda-appid)

_install-system name appid:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{ base-dir / 'bin' / name }}
    install -Dm0644 {{ name / 'data' / appid + '.desktop' }} {{ base-dir / 'share' / 'applications' / appid + '.desktop' }}
    install -Dm0644 {{ name / 'data' / 'icons' / appid + '.svg' }} {{ base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg' }}

_install-user name appid:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{ user-base-dir / 'bin' / name }}
    install -Dm0644 {{ name / 'data' / appid + '.desktop' }} {{ user-base-dir / 'share' / 'applications' / appid + '.desktop' }}
    install -Dm0644 {{ name / 'data' / 'icons' / appid + '.svg' }} {{ user-base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg' }}

_uninstall-system name appid:
    rm -f {{ base-dir / 'bin' / name }} {{ base-dir / 'share' / 'applications' / appid + '.desktop' }} {{ base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg' }}

_uninstall-user name appid:
    rm -f {{ user-base-dir / 'bin' / name }} {{ user-base-dir / 'share' / 'applications' / appid + '.desktop' }} {{ user-base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg' }}
