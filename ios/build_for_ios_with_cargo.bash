#!/usr/bin/env bash
set -euvx

export PATH="/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH:$HOME/.cargo/bin"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-16.0}"

cd "$SRCROOT/.."

if [[ "$CONFIGURATION" != "Debug" ]]; then
    CARGO_PROFILE=release
    MAYBE_RELEASE=--release
else
    CARGO_PROFILE=debug
    MAYBE_RELEASE=
fi

export CARGO_PROFILE_RELEASE_DEBUG="${CARGO_PROFILE_RELEASE_DEBUG:-1}"
export CARGO_TARGET_DIR="$DERIVED_FILE_DIR/cargo"

IS_SIMULATOR=0
if [[ "${LLVM_TARGET_TRIPLE_SUFFIX-}" == "-simulator" ]]; then
    IS_SIMULATOR=1
fi

executables=()
for arch in $ARCHS; do
    case "$arch" in
        arm64)
            if [[ $IS_SIMULATOR -eq 0 ]]; then
                CARGO_TARGET=aarch64-apple-ios
            else
                CARGO_TARGET=aarch64-apple-ios-sim
            fi
            ;;
        x86_64)
            export CFLAGS_x86_64_apple_ios="-target x86_64-apple-ios"
            CARGO_TARGET=x86_64-apple-ios
            ;;
        *)
            echo "Unsupported iOS architecture: $arch" >&2
            exit 1
            ;;
    esac

    cargo build $MAYBE_RELEASE --target "$CARGO_TARGET" --bin kunianki
    executables+=("$CARGO_TARGET_DIR/$CARGO_TARGET/$CARGO_PROFILE/kunianki")
done

lipo -create -output "$TARGET_BUILD_DIR/$EXECUTABLE_PATH" "${executables[@]}"

if [[ -n "${DWARF_DSYM_FOLDER_PATH:-}" && -n "${DWARF_DSYM_FILE_NAME:-}" ]]; then
    mkdir -p "$DWARF_DSYM_FOLDER_PATH"
    dsymutil "$TARGET_BUILD_DIR/$EXECUTABLE_PATH" -o "$DWARF_DSYM_FOLDER_PATH/$DWARF_DSYM_FILE_NAME"
fi

if [[ $IS_SIMULATOR -eq 0 && "${CODE_SIGNING_ALLOWED:-YES}" != "NO" && -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ]]; then
    ENTITLEMENTS_FILE="${TARGET_TEMP_DIR}/${PRODUCT_NAME}.app.xcent"
    if [[ -s "$ENTITLEMENTS_FILE" ]]; then
        codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" \
            --entitlements "$ENTITLEMENTS_FILE" \
            "${TARGET_BUILD_DIR}/${EXECUTABLE_PATH}"
    else
        codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" \
            "${TARGET_BUILD_DIR}/${EXECUTABLE_PATH}"
    fi
fi
