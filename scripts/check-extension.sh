#!/usr/bin/env bash
#
# Validate the Zed extension without installing it.
#
#   1. `extension.toml` and `languages/http-request/config.toml` parse, and
#      carry the keys Zed requires. A typo here only bites at install time.
#   2. Every `.scm` query compiles against the *pinned* grammar revision.
#      Zed clones the grammar at that rev, so a query naming a node the
#      grammar no longer has is a broken install — catch it here instead.
#
# Needs: python3 (>=3.11, for tomllib), git, npx.
#
set -euo pipefail

cd "$(dirname "$0")/.."

TREE_SITTER_CLI_VERSION="0.25.10"
EXTENSION_DIR="extension"
LANGUAGE_DIR="$EXTENSION_DIR/languages/http-request"

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

echo "==> Validating $EXTENSION_DIR/extension.toml"
# Prints `<grammar-repo> <grammar-rev>` on success; exits non-zero on any
# missing/malformed key, so `read` below never sees half-checked data.
read -r grammar_repo grammar_rev < <(python3 - "$EXTENSION_DIR" "$LANGUAGE_DIR" <<'PY'
import pathlib
import re
import sys
import tomllib

extension_dir = pathlib.Path(sys.argv[1])
language_dir = pathlib.Path(sys.argv[2])
errors = []


def load(path):
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except FileNotFoundError:
        errors.append(f"{path}: missing")
    except tomllib.TOMLDecodeError as error:
        errors.append(f"{path}: not valid TOML: {error}")
    return None


extension = load(extension_dir / "extension.toml")
language = load(language_dir / "config.toml")

if extension is not None:
    for key in ("id", "name", "version", "schema_version", "description", "authors"):
        if key not in extension:
            errors.append(f"extension.toml: missing required key '{key}'")

    for relative in extension.get("languages", []):
        if not (extension_dir / relative).is_dir():
            errors.append(f"extension.toml: languages entry '{relative}' is not a directory")

    grammars = extension.get("grammars", {})
    if not grammars:
        errors.append("extension.toml: no [grammars.*] table")
    for name, grammar in grammars.items():
        if "repository" not in grammar:
            errors.append(f"extension.toml: [grammars.{name}] has no repository")
        rev = grammar.get("rev", "")
        # Zed clones at this rev verbatim; a branch name or short SHA is not
        # reproducible, so require the full 40-character commit SHA.
        if not re.fullmatch(r"[0-9a-f]{40}", rev):
            errors.append(
                f"extension.toml: [grammars.{name}] rev '{rev}' is not a 40-character commit SHA"
            )

if language is not None and extension is not None:
    grammar = language.get("grammar")
    if grammar is None:
        errors.append("languages/http-request/config.toml: missing 'grammar'")
    elif grammar not in extension.get("grammars", {}):
        errors.append(
            f"languages/http-request/config.toml: grammar '{grammar}' has no "
            f"[grammars.{grammar}] table in extension.toml"
        )
    if not language.get("path_suffixes"):
        errors.append("languages/http-request/config.toml: missing 'path_suffixes'")

if errors:
    for error in errors:
        print(f"::error::{error}", file=sys.stderr)
    sys.exit(1)

only = next(iter(extension["grammars"].values()))
print(only["repository"], only["rev"])
PY
)
echo "    OK — grammar $grammar_repo @ $grammar_rev"

echo "==> Cloning the pinned grammar"
git init -q "$workdir/grammar"
git -C "$workdir/grammar" remote add origin "$grammar_repo"
git -C "$workdir/grammar" fetch -q --depth 1 origin "$grammar_rev"
git -C "$workdir/grammar" checkout -q FETCH_HEAD

# The grammar repo ships a generated src/parser.c but no tree-sitter.json, which
# the CLI needs to locate the language. Supply a minimal one rather than
# regenerating the parser (which would test the CLI's grammar.js, not the
# parser Zed actually compiles).
cat > "$workdir/grammar/tree-sitter.json" <<JSON
{
  "grammars": [
    {
      "name": "http_request",
      "camelcase": "HttpRequest",
      "scope": "source.http",
      "path": ".",
      "file-types": ["http", "rest"]
    }
  ],
  "metadata": { "version": "0.0.0" }
}
JSON

echo "==> Compiling queries against the pinned grammar"
sample="$workdir/grammar/sample.http"
cp examples/requests.http "$sample"

status=0
for query in "$LANGUAGE_DIR"/*.scm; do
  name="$(basename "$query")"
  cp "$query" "$workdir/grammar/$name"
  if (cd "$workdir/grammar" && npx -y "tree-sitter-cli@$TREE_SITTER_CLI_VERSION" \
        query "$name" sample.http >/dev/null 2>"$workdir/$name.err"); then
    echo "    OK — $name"
  else
    echo "::error::$name does not compile against grammar $grammar_rev" >&2
    # The CLI always warns that no parser directories are configured; we run
    # from inside the grammar dir on purpose, so that warning is just noise.
    sed -e '/^Warning: You have not configured any parser directories!$/,/^$/d' \
        -e 's/^/    /' "$workdir/$name.err" >&2
    status=1
  fi
done

if [[ $status -eq 0 ]]; then
  echo "OK: extension metadata and queries are valid"
fi
exit $status
