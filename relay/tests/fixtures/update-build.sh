#!/bin/sh
# Stands in for a downloaded relay build in tests/update.rs. Signed with a
# throwaway key (tests/fixtures/update-build.pub), never the release key.
case "$1" in
  --version) echo "reamesh-relay 99.0.0" ;;
  *) echo "started $*" >> "$REAMESH_TEST_OUT" ;;
esac
