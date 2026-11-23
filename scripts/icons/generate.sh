#!/usr/bin/env bash
# Generates the committed PNG/ICNS icons from artwork/*.svg; needs rsvg-convert (librsvg2-bin) and png2icns (icnsutils).
# Meant to run in the build VM, never on the Mac.

set -euo pipefail

die() {
    printf '==> ERROR: %s\n' "$*" >&2
    exit 1
}

[[ $# -eq 1 && -n "${1:-}" ]] || die "usage: generate.sh <out-dir>"
[[ ! -e "$1" && ! -L "$1" ]] || die "out-dir already exists: $1"

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
ART="$ROOT/artwork"
MANIFEST="$ROOT/scripts/icons/manifest.txt"

type -P rsvg-convert >/dev/null || die "rsvg-convert not on PATH (apt package librsvg2-bin)"
type -P png2icns >/dev/null || die "png2icns not on PATH (apt package icnsutils)"

[[ -f "$MANIFEST" ]] || die "manifest not found: $MANIFEST"
for svg in shellcue-icon.svg shellcue-icon-small.svg shellcue-menubar.svg shellcue-dmg-background.svg; do
    [[ -f "$ART/$svg" ]] || die "artwork not found: $ART/$svg"
done

mkdir -p -- "$1"
OUT="$(cd -- "$1" && pwd)"

WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

render() {
    local svg="$1" width="$2" height="$3" dest="$4"
    mkdir -p -- "$(dirname -- "$dest")"
    printf '==> render %s %sx%s -> %s\n' "${svg##*/}" "$width" "$height" "$dest"
    rsvg-convert -w "$width" -h "$height" -o "$dest" -- "$svg"
}

render_tray() {
    local colour="$1" dest="$2"
    local tmp="$WORK/menubar-${colour#\#}.svg"
    sed -e "s/currentColor/$colour/g" -- "$ART/shellcue-menubar.svg" >"$tmp"
    if grep -q -F -e currentColor -- "$tmp"; then
        die "currentColor still present after recolouring to $colour"
    fi
    grep -q -F -e "$colour" -- "$tmp" || die "$colour missing after recolouring"
    render "$tmp" 36 36 "$dest"
}

render "$ART/shellcue-icon.svg" 512 512 "$OUT/crates/fig_desktop/icons/icon.png"
render "$ART/shellcue-icon-small.svg" 32 32 "$OUT/crates/fig_desktop/icons/32x32.png"
render "$ART/shellcue-icon.svg" 128 128 "$OUT/crates/fig_desktop/icons/128x128.png"
render "$ART/shellcue-icon.svg" 256 256 "$OUT/crates/fig_desktop/icons/128x128@2x.png"

render_tray '#000000' "$OUT/crates/fig_desktop/icons/icon-monochrome.png"
render_tray '#FFFFFF' "$OUT/crates/fig_desktop/icons/icon-monochrome-light.png"

icns_members=()
for size in 16 32; do
    render "$ART/shellcue-icon-small.svg" "$size" "$size" "$WORK/icns/icon_$size.png"
    icns_members+=("$WORK/icns/icon_$size.png")
done
for size in 128 256 512 1024; do
    render "$ART/shellcue-icon.svg" "$size" "$size" "$WORK/icns/icon_$size.png"
    icns_members+=("$WORK/icns/icon_$size.png")
done
printf '==> png2icns %s\n' "$WORK/icon.icns"
png2icns "$WORK/icon.icns" "${icns_members[@]}"
[[ -s "$WORK/icon.icns" ]] || die "png2icns produced no output"
if type -P icns2png >/dev/null; then
    icns2png -l "$WORK/icon.icns" || printf '==> warning: icns2png listing failed\n' >&2
fi
for dest in crates/fig_desktop/icons/icon.icns bundle/dmg/VolumeIcon.icns crates/fig_input_method/resources/product_icon.icns; do
    mkdir -p -- "$(dirname -- "$OUT/$dest")"
    cp -- "$WORK/icon.icns" "$OUT/$dest"
done

render "$ART/shellcue-dmg-background.svg" 660 400 "$OUT/bundle/dmg/background.png"
render "$ART/shellcue-dmg-background.svg" 1320 800 "$OUT/bundle/dmg/background@2x.png"

printf '==> verify against %s\n' "$MANIFEST"
manifest_paths=()
while read -r kind width height path || [[ -n "$kind" ]]; do
    [[ ( "$kind" == png || "$kind" == icns ) && -n "$width" && -n "$height" && -n "$path" ]] \
        || die "bad manifest line: $kind $width $height $path"
    [[ -f "$OUT/$path" && -s "$OUT/$path" ]] || die "missing or empty: $path"
    manifest_paths+=("$path")
done <"$MANIFEST"
(( ${#manifest_paths[@]} > 0 )) || die "manifest is empty"

listed="$(printf '%s\n' "${manifest_paths[@]}" | LC_ALL=C sort)"
present="$(cd -- "$OUT" && find . -type f -printf '%P\n' | LC_ALL=C sort)"
extra="$(LC_ALL=C comm -13 <(printf '%s\n' "$listed") <(printf '%s\n' "$present"))"
[[ -z "$extra" ]] || die "out-dir has files not in the manifest: ${extra//$'\n'/ }"

cd -- "$OUT"
for path in "${manifest_paths[@]}"; do
    sha256sum -- "$path"
done
