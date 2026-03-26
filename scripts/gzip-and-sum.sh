#!/bin/bash

set -e
set -x

gzip_and_sum() {
    gzip -c "$1" > "$2"
    sha256sum "$2" > "$2.sha256"
}
