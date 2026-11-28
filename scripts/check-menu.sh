#!/bin/sh

set -eu

if [ "$#" -gt 1 ]; then
    echo "usage: $0 [menu.json]" >&2
    exit 2
fi

MENU_JSON="${1:-kindle-extension/rust_x11_hello/menu.json}"

if [ ! -r "$MENU_JSON" ]; then
    echo "menu file is not readable: $MENU_JSON" >&2
    exit 1
fi

jq -e '
    type == "object" and
    (keys == ["items"]) and
    (.items | type == "array" and length > 0) and
    ([.items[].name] | length == (unique | length)) and
    all(.items[];
        type == "object" and
        (keys == ["items", "name", "priority"]) and
        (.name | type == "string" and length > 0) and
        (.priority | type == "number") and
        (.items | type == "array" and length > 0) and
        all(
            .items[];
            type == "object" and
            (keys == ["action", "name"]) and
            (.name | type == "string" and length > 0) and
            (.action | type == "string" and length > 0)
        ) and
        ([.items[].name] | length == (unique | length)) and
        ([.items[].action] | length == (unique | length))
    )
' "$MENU_JSON" >/dev/null
