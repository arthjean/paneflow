#!/usr/bin/env bash
set -euo pipefail

APP="${1:-dist/PaneFlow.app}"

[ -d "$APP" ] || { echo "error: bundle not found: $APP" >&2; exit 1; }

: "${APPLE_ID:?APPLE_ID env var is required}"
: "${APPLE_APP_SPECIFIC_PASSWORD:?APPLE_APP_SPECIFIC_PASSWORD env var is required}"
: "${APPLE_TEAM_ID:?APPLE_TEAM_ID env var is required}"

ZIP="${APP%.app}.zip"

cleanup() {
    rm -f "$ZIP"
}
trap cleanup EXIT

ditto -c -k --keepParent "$APP" "$ZIP"

echo "Submitting $ZIP to notarytool..."
SUBMIT_JSON="$(xcrun notarytool submit "$ZIP" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --output-format json)"

SUBMISSION_ID="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])' <<< "$SUBMIT_JSON")"

echo "Submission ID: $SUBMISSION_ID"
echo "(If this job is killed mid-poll, recover status from any Mac with:"
echo "    xcrun notarytool info $SUBMISSION_ID --apple-id <APPLE_ID> --team-id $APPLE_TEAM_ID --password <APP_SPECIFIC_PASSWORD>)"

POLL_INTERVAL=30
MAX_WAIT_SECONDS=$((90 * 60))
START_TIME=$(date +%s)

while true; do
    INFO_JSON="$(xcrun notarytool info "$SUBMISSION_ID" \
        --apple-id "$APPLE_ID" \
        --password "$APPLE_APP_SPECIFIC_PASSWORD" \
        --team-id "$APPLE_TEAM_ID" \
        --output-format json)"
    STATUS="$(python3 -c 'import json,sys; print(json.load(sys.stdin).get("status", "Unknown"))' <<< "$INFO_JSON")"

    NOW=$(date +%s)
    ELAPSED=$((NOW - START_TIME))
    ELAPSED_FMT="$(printf '%02d:%02d' $((ELAPSED / 60)) $((ELAPSED % 60)))"

    case "$STATUS" in
        Accepted)
            echo "[+${ELAPSED_FMT}] Accepted by Apple"
            break
            ;;
        Invalid|Rejected)
            echo "::error title=Notarization::Apple rejected submission (status=$STATUS, id=$SUBMISSION_ID)"
            echo "--- notarytool log $SUBMISSION_ID ---" >&2
            xcrun notarytool log "$SUBMISSION_ID" \
                --apple-id "$APPLE_ID" \
                --password "$APPLE_APP_SPECIFIC_PASSWORD" \
                --team-id "$APPLE_TEAM_ID" \
                >&2 || echo "(failed to retrieve log - Apple may still be processing)" >&2
            exit 1
            ;;
        "In Progress")
            echo "[+${ELAPSED_FMT}] In Progress... (next poll in ${POLL_INTERVAL}s)"
            ;;
        *)
            echo "[+${ELAPSED_FMT}] Unexpected status: $STATUS - continuing to poll"
            ;;
    esac

    if [ "$ELAPSED" -ge "$MAX_WAIT_SECONDS" ]; then
        echo "::error title=Notarization timeout::Submission $SUBMISSION_ID still pending after $((MAX_WAIT_SECONDS / 60)) minutes."
        echo "::error::Apple's notary backend is in deep queue. Recover later with:"
        echo "::error::  xcrun notarytool info $SUBMISSION_ID --apple-id <APPLE_ID> --team-id $APPLE_TEAM_ID --password <APP_SPECIFIC_PASSWORD>"
        echo "::error::If the submission later reaches Accepted, staple manually with:"
        echo "::error::  xcrun stapler staple $APP"
        exit 1
    fi

    sleep "$POLL_INTERVAL"
done

xcrun stapler staple "$APP"
xcrun stapler validate "$APP"

spctl --assess --type exec --verbose "$APP"

echo "Notarized + stapled: $APP (submission_id=$SUBMISSION_ID)"
