#!/usr/bin/env bash

# Managed by componentized/build and synced into each repository by `make update-build`. Change it
# in https://github.com/componentized/build, `make check-build` fails when a copy is changed.

# Create a commit of the staged changes with the GitHub REST API, signed by GitHub, for a GitHub App
# token that can't push.
#
#   GH_TOKEN=<token> APP_SLUG=<slug> build/scripts/create-commit.sh <base> <message>
#
# Prints the commit, a child of <base>, the commit the changes are staged against. No branch is moved
# to it. The changes are only staged locally to find the changed files and their modes.
#
# The commit is authored, committed and signed off (DCO) by the app's bot. GitHub only signs commits
# it attributes entirely to the app, so the author and committer are left for GitHub to fill in. The
# user who triggered the workflow is credited as a co-author, with their GitHub noreply email so their
# email address isn't exposed. A run started by the app has no one else to credit.
#
# GH_TOKEN           a token for the app, with contents write permission, and workflows write
#                    permission to change workflow files
# APP_SLUG           the slug of the app, e.g. custodian
# GITHUB_REPOSITORY  the repository to create the commit in, e.g. componentized/http
# GITHUB_ACTOR       the user who triggered the workflow, credited as a co-author
# GITHUB_ACTOR_ID    the id of the user
# RUNNER_TEMP        a directory for temporary files

set -euo pipefail

base="${1:?usage: $0 <base> <message>}"
subject="${2:?usage: $0 <base> <message>}"
: "${GH_TOKEN:?GH_TOKEN is undefined}"
: "${APP_SLUG:?APP_SLUG is undefined}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is undefined}"

api="repos/${GITHUB_REPOSITORY}"
temp="${RUNNER_TEMP:-$(mktemp -d)}"

# a blob for each changed file, or a null sha for a deleted file
entries="${temp}/tree-entries.json"
echo '[]' > "${entries}"
git diff --cached --no-renames --name-status "${base}" | while IFS=$'\t' read -r status path ; do
  if [ "${status}" = "D" ] ; then
    entry=$( jq -n --arg path "${path}" '{path: $path, mode: "100644", type: "blob", sha: null}' )
  else
    mode=$( git ls-files --stage -- "${path}" | cut -d' ' -f1 )
    sha=$( base64 < "${path}" | tr -d '\n' | jq -Rs '{encoding: "base64", content: .}' | gh api --method POST "${api}/git/blobs" --input - --jq .sha )
    entry=$( jq -n --arg path "${path}" --arg mode "${mode}" --arg sha "${sha}" '{path: $path, mode: $mode, type: "blob", sha: $sha}' )
  fi
  jq --argjson entry "${entry}" '. + [$entry]' "${entries}" > "${entries}.tmp" && mv "${entries}.tmp" "${entries}"
  echo "${status} ${path}" >&2
done
tree=$( jq --arg base "$( git rev-parse "${base}^{tree}" )" '{base_tree: $base, tree: .}' "${entries}" | gh api --method POST "${api}/git/trees" --input - --jq .sha )

bot="${APP_SLUG}[bot]"
bot_email="$( gh api "users/${bot}" --jq .id )+${bot}@users.noreply.github.com"
message=$( printf '%s\n\nSigned-off-by: %s <%s>' "${subject}" "${bot}" "${bot_email}" )
if [ -n "${GITHUB_ACTOR:-}" ] && [ "${GITHUB_ACTOR}" != "${bot}" ] ; then
  name=$( gh api "users/${GITHUB_ACTOR}" --jq '.name // .login' )
  name="${name:-${GITHUB_ACTOR}}"
  email="${GITHUB_ACTOR_ID}+${GITHUB_ACTOR}@users.noreply.github.com"
  message=$( printf '%s\nCo-authored-by: %s <%s>' "${message}" "${name}" "${email}" )
fi
read -r commit verified < <( jq -n \
  --arg message "${message}" \
  --arg tree "${tree}" --arg parent "${base}" \
  '{message: $message, tree: $tree, parents: [$parent]}' \
  | gh api --method POST "${api}/git/commits" --input - --jq '"\(.sha) \(.verification.verified)"' )
echo "created commit ${commit}" >&2
# the branch requires signed commits, fail before pushing rather than after
if [ "${verified}" != "true" ] ; then
  echo "::error::the commit '${commit}' was not created, or was not signed by GitHub" >&2
  exit 1
fi

echo "${commit}"
