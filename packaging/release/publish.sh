#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd dist
tag="v$VERSION"
cp -- "$CHECKSUM_BUNDLE" "HelloNeedle-$VERSION-Release.sigstore.json"
gh attestation verify SHA256SUMS --repo "$GH_REPO" --bundle "HelloNeedle-$VERSION-Release.sigstore.json" \
    --source-digest "$GITHUB_SHA" --signer-workflow "$GH_REPO/.github/workflows/installers-release.yml"
sha256sum -c SHA256SUMS

# A new tag may be created at publication. Reusing a tag for another commit would misidentify the source.
ref=$(git ls-remote origin "refs/tags/$tag" "refs/tags/$tag^{}")
if [[ -n $ref ]]; then
    target=$(awk 'NR == 1 {sha=$1} /\^\{\}$/ {sha=$1} END {print sha}' <<< "$ref")
    [[ $target == "$GITHUB_SHA" ]] || { echo "Tag $tag already identifies another revision." >&2; exit 1; }
fi
release=$(gh api --paginate "repos/$GH_REPO/releases?per_page=100" |
    jq -cs --arg tag "$tag" '[.[][] | select(.tag_name == $tag)][0] // empty')
if [[ -z $release ]]; then
    notes=$(mktemp)
    sed -e "s/@VERSION@/$VERSION/g" -e "s|@REPOSITORY@|$GH_REPO|g" ../packaging/release/notes.md > "$notes"
    gh release create "$tag" --draft --target "$GITHUB_SHA" --title "Hello Needle $VERSION" --notes-file "$notes"
    release=$(gh release view "$tag" --json databaseId,isDraft,targetCommitish |
        jq '{id:.databaseId,draft:.isDraft,target_commitish:.targetCommitish}')
fi
[[ $(jq -r .target_commitish <<< "$release") == "$GITHUB_SHA" ]] || { echo 'A release already points elsewhere.' >&2; exit 1; }
release_id=$(jq -er .id <<< "$release")
asset_digests() {
    gh api --paginate "repos/$GH_REPO/releases/$release_id/assets?per_page=100" | jq -scr '.[][] | [.name,.digest] | @tsv' | sort
}
local_digests=$(for path in *; do printf '%s\tsha256:%s\n' "$path" "$(sha256sum "$path" | cut -d' ' -f1)"; done | sort)
if [[ $(jq -r .draft <<< "$release") == true ]]; then
    gh release upload "$tag" ./* --clobber
    [[ $(asset_digests) == "$local_digests" ]] || { echo 'Draft assets differ from verified local bytes.' >&2; exit 1; }
    if [[ $PUBLISH == true ]]; then gh release edit "$tag" --draft=false --latest; fi
else
    # Reruns may verify an existing release, but never replace published downloads.
    [[ $PUBLISH == true && $(asset_digests) == "$local_digests" ]] || { echo 'This version has already been published.' >&2; exit 1; }
fi
release=$(gh release view "$tag" --json url,isDraft,isImmutable)
echo "$release" | jq .
if [[ $(jq -r '.isDraft' <<< "$release") == false && $(jq -r '.isImmutable' <<< "$release") == true ]]; then
    verified=false
    for delay in 1 3 7 15 30 30; do
        if gh release verify "$tag"; then verified=true; break; fi
        sleep "$delay"
    done
    [[ $verified == true ]]
    gh release verify-asset "$tag" "HelloNeedle-$VERSION-Windows-x64-Setup.exe"
    gh release verify-asset "$tag" "HelloNeedle-$VERSION-macOS-Universal.dmg"
fi
