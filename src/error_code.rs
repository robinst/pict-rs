#[derive(Debug, serde::Serialize)]
#[serde(transparent)]
pub(crate) struct ErrorCode {
    code: &'static str,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub(crate) struct OwnedErrorCode {
    code: String,
}

impl ErrorCode {
    pub(crate) fn into_owned(self) -> OwnedErrorCode {
        OwnedErrorCode {
            code: self.code.to_string(),
        }
    }

    pub(crate) const COMMAND_TIMEOUT: ErrorCode = ErrorCode {
        code: "command-timeout",
    };
    pub(crate) const COMMAND_ERROR: ErrorCode = ErrorCode {
        code: "command-error",
    };
    pub(crate) const COMMAND_FAILURE: ErrorCode = ErrorCode {
        code: "command-failure",
    };
    pub(crate) const OLD_REPO_ERROR: ErrorCode = ErrorCode {
        code: "old-repo-error",
    };
    pub(crate) const NOT_FOUND: ErrorCode = ErrorCode { code: "not-found" };
    pub(crate) const FILE_IO_ERROR: ErrorCode = ErrorCode {
        code: "file-io-error",
    };
    pub(crate) const FILE_EXISTS: ErrorCode = ErrorCode {
        code: "file-exists",
    };
    pub(crate) const FORMAT_FILE_ID_ERROR: ErrorCode = ErrorCode {
        code: "format-file-id-error",
    };
    pub(crate) const OBJECT_REQUEST_ERROR: ErrorCode = ErrorCode {
        code: "object-request-error",
    };
    pub(crate) const OBJECT_IO_ERROR: ErrorCode = ErrorCode {
        code: "object-io-error",
    };
    pub(crate) const PARSE_OBJECT_ID_ERROR: ErrorCode = ErrorCode {
        code: "parse-object-id-error",
    };
    pub(crate) const PANIC: ErrorCode = ErrorCode { code: "panic" };
    pub(crate) const ALREADY_CLAIMED: ErrorCode = ErrorCode {
        code: "already-claimed",
    };
    pub(crate) const SLED_ERROR: ErrorCode = ErrorCode { code: "sled-error" };
    pub(crate) const POSTGRES_ERROR: ErrorCode = ErrorCode {
        code: "postgres-error",
    };
    pub(crate) const EXTRACT_DETAILS: ErrorCode = ErrorCode {
        code: "extract-details",
    };
    pub(crate) const EXTRACT_UPLOAD_RESULT: ErrorCode = ErrorCode {
        code: "extract-upload-result",
    };
    pub(crate) const PUSH_JOB: ErrorCode = ErrorCode { code: "push-job" };
    pub(crate) const EXTRACT_JOB: ErrorCode = ErrorCode {
        code: "extract-job",
    };
    pub(crate) const CONFLICTED_RECORD: ErrorCode = ErrorCode {
        code: "conflicted-record",
    };
    pub(crate) const COMMAND_NOT_FOUND: ErrorCode = ErrorCode {
        code: "command-not-found",
    };
    pub(crate) const COMMAND_PERMISSION_DENIED: ErrorCode = ErrorCode {
        code: "command-permission-denied",
    };
    pub(crate) const FILE_UPLOAD_ERROR: ErrorCode = ErrorCode {
        code: "file-upload-error",
    };
    pub(crate) const IO_ERROR: ErrorCode = ErrorCode { code: "io-error" };
    pub(crate) const VALIDATE_WIDTH: ErrorCode = ErrorCode {
        code: "validate-width",
    };
    pub(crate) const VALIDATE_HEIGHT: ErrorCode = ErrorCode {
        code: "validate-height",
    };
    pub(crate) const VALIDATE_AREA: ErrorCode = ErrorCode {
        code: "validate-area",
    };
    pub(crate) const VALIDATE_FRAMES: ErrorCode = ErrorCode {
        code: "validate-frames",
    };
    pub(crate) const VALIDATE_FILE_EMPTY: ErrorCode = ErrorCode {
        code: "validate-file-empty",
    };
    pub(crate) const VALIDATE_FILE_SIZE: ErrorCode = ErrorCode {
        code: "validate-file-size",
    };
    pub(crate) const VIDEO_DISABLED: ErrorCode = ErrorCode {
        code: "video-disabled",
    };
    pub(crate) const HTTP_CLIENT_ERROR: ErrorCode = ErrorCode {
        code: "http-client-error",
    };
    pub(crate) const DOWNLOAD_FILE_ERROR: ErrorCode = ErrorCode {
        code: "download-file-error",
    };
    pub(crate) const READ_ONLY: ErrorCode = ErrorCode { code: "read-only" };
    pub(crate) const INVALID_FILE_EXTENSION: ErrorCode = ErrorCode {
        code: "invalid-file-extension",
    };
    pub(crate) const INVALID_PROCESS_PATH: ErrorCode = ErrorCode {
        code: "invalid-process-path",
    };
    pub(crate) const PROCESS_SEMAPHORE_CLOSED: ErrorCode = ErrorCode {
        code: "process-semaphore-closed",
    };
    pub(crate) const VALIDATE_NO_FILES: ErrorCode = ErrorCode {
        code: "validate-no-files",
    };
    pub(crate) const PROXY_NOT_FOUND: ErrorCode = ErrorCode {
        code: "proxy-not-found",
    };
    pub(crate) const ALIAS_NOT_FOUND: ErrorCode = ErrorCode {
        code: "alias-not-found",
    };
    pub(crate) const LOST_FILE: ErrorCode = ErrorCode { code: "lost-file" };
    pub(crate) const INVALID_DELETE_TOKEN: ErrorCode = ErrorCode {
        code: "invalid-delete-token",
    };
    pub(crate) const DUPLICATE_ALIAS: ErrorCode = ErrorCode {
        code: "duplicate-alias",
    };
    pub(crate) const RANGE_NOT_SATISFIABLE: ErrorCode = ErrorCode {
        code: "range-not-satisfiable",
    };
    pub(crate) const STREAM_TOO_SLOW: ErrorCode = ErrorCode {
        code: "stream-too-slow",
    };
    pub(crate) const UNKNOWN_ERROR: ErrorCode = ErrorCode {
        code: "unknown-error",
    };
    pub(crate) const FAILED_EXTERNAL_VALIDATION: ErrorCode = ErrorCode {
        code: "failed-external-validation",
    };
    pub(crate) const INVALID_JOB: ErrorCode = ErrorCode {
        code: "invalid-job",
    };
}
