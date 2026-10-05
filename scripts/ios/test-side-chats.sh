#!/bin/bash
# Side-chat captures on one installed iPhone/iPad simulator: the branch
# sheet, the session menu's creation actions, the fork seam, and search
# grouping. Screenshots are the UI tests' XCTAttachments, exported from the
# result bundle.
#
# Usage (from a worktree): scripts/ios/test-side-chats.sh <simulator-udid> [output-dir]
set -euo pipefail

ROOT="${IOS_PROJECT_ROOT:-$(git rev-parse --show-toplevel)}"
DEVICE="${1:?Pass an installed iPhone or iPad simulator UDID}"
OUT="${2:-$ROOT/target/ios-side-chats/$DEVICE}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
DERIVED="${IOS_SIDE_CHATS_DERIVED_DATA:-$ROOT/target/ios-side-chats/DerivedData}"

if [ -e "$OUT/tests.xcresult" ]; then
  echo "Choose a new output directory: $OUT/tests.xcresult already exists" >&2
  exit 1
fi

set +e
xcodebuild -project "$ROOT/apps/ios/Zeron.xcodeproj" -scheme Zeron \
  -destination "platform=iOS Simulator,id=$DEVICE" \
  -derivedDataPath "$DERIVED" -resultBundlePath "$OUT/tests.xcresult" \
  -parallel-testing-enabled NO CODE_SIGNING_ALLOWED=NO \
  -only-testing:ZeronUITests/SessionFlowTests/testSideChatsSheetAndForkSeam \
  -only-testing:ZeronUITests/SessionFlowTests/testNewSideChatFromMenu \
  -only-testing:ZeronUITests/SessionFlowTests/testForkToSideChatFromMenu \
  -only-testing:ZeronUITests/SessionFlowTests/testSearchFindsSideChats \
  test 2>&1 | tee "$OUT/xcodebuild.log"
STATUS=${PIPESTATUS[0]}
set -e

rm -rf "$OUT/attachments"
xcrun xcresulttool export attachments --path "$OUT/tests.xcresult" \
  --output-path "$OUT/attachments"
echo "Results: $OUT/tests.xcresult"
echo "Screenshots: $OUT/attachments"
exit "$STATUS"
