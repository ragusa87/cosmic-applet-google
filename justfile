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
taxi-name := 'cosmic-applet-taxi'
taxi-appid := 'com.github.ragusa87.CosmicAppletTaxi'

default: build-release

clean:
    cargo clean

build-debug crate='' *args:
    cargo build {{ if crate == '' { '' } else { '-p ' + crate } }} {{args}}

# Release build. With no arg, builds the whole workspace. Pass a crate name to
# build only that applet, e.g. `just build-release cosmic-applet-taxi`.
build-release crate='' *args: (build-debug crate '--release' args)

# Fast release build of a single applet — optimized but no LTO. Outputs to
# target/release-fast/<crate>. Example: `just build-fast cosmic-applet-taxi`.
build-fast crate *args:
    cargo build --profile=release-fast -p {{crate}} {{args}}

# Install a single applet from the release-fast profile into ~/.local
# without rebuilding the other two. Example: `just install-fast cosmic-applet-taxi`.
install-fast crate *args: (build-fast crate args)
    @just _install-fast {{crate}} $(just _appid-of {{crate}})

_install-fast name appid:
    install -Dm0755 {{ cargo-target-dir / 'release-fast' / name }} {{ user-base-dir / 'bin' / name }}
    install -Dm0644 {{ name / 'data' / appid + '.desktop' }} {{ user-base-dir / 'share' / 'applications' / appid + '.desktop' }}
    install -Dm0644 {{ name / 'data' / 'icons' / appid + '.svg' }} {{ user-base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg' }}

_appid-of crate:
    @case "{{crate}}" in \
        cosmic-applet-gmail)         echo "{{gmail-appid}}" ;; \
        cosmic-applet-google-agenda) echo "{{agenda-appid}}" ;; \
        cosmic-applet-taxi)          echo "{{taxi-appid}}" ;; \
        *) echo "unknown crate: {{crate}}" >&2; exit 1 ;; \
    esac

check *args:
    cargo clippy --workspace --all-features {{args}} -- -W clippy::pedantic

run-gmail *args:
    env RUST_BACKTRACE=full cargo run --release -p cosmic-applet-gmail {{args}}

run-agenda *args:
    env RUST_BACKTRACE=full cargo run --release -p cosmic-applet-google-agenda {{args}}

run-taxi *args:
    env RUST_BACKTRACE=full cargo run --release -p cosmic-applet-taxi {{args}}

refresh:
    -pkill -USR2 -f cosmic-applet-gmail            # poll Gmail right now
    -pkill -USR2 -f cosmic-applet-google-agenda    # refetch calendar right now
    -pkill -USR2 -f cosmic-applet-taxi             # reload taxi state right now

# Fast-build a single applet, user-install it, then restart cosmic-panel so it
# picks up the new binary. Example: `just debug-run cosmic-applet-taxi`.
debug-run crate *args: (install-fast crate args)
    -pkill -x cosmic-panel

install: \
    (_install-system gmail-name gmail-appid) \
    (_install-system agenda-name agenda-appid) \
    (_install-system taxi-name taxi-appid)

install-user: \
    (_install-user gmail-name gmail-appid) \
    (_install-user agenda-name agenda-appid) \
    (_install-user taxi-name taxi-appid)

uninstall: \
    (_uninstall-system gmail-name gmail-appid) \
    (_uninstall-system agenda-name agenda-appid) \
    (_uninstall-system taxi-name taxi-appid)

uninstall-user: \
    (_uninstall-user gmail-name gmail-appid) \
    (_uninstall-user agenda-name agenda-appid) \
    (_uninstall-user taxi-name taxi-appid)

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
