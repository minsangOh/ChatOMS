import type { IpcErrorDto, IpcRetry, IpcSeverity } from "./types";

const IPC_SEVERITIES: readonly IpcSeverity[] = ["info", "warning", "error", "critical"];
const IPC_RETRIES: readonly IpcRetry[] = [
  "never",
  "immediate",
  "afterUserAction",
  "afterStateRefresh",
];

export class FrontendError extends Error {
  readonly code: string;
  readonly severity: IpcSeverity;
  readonly retry: IpcRetry;

  constructor(error: IpcErrorDto) {
    super(error.message);
    this.name = "FrontendError";
    this.code = error.code;
    this.severity = error.severity;
    this.retry = error.retry;
  }
}

export function isIpcErrorDto(value: unknown): value is IpcErrorDto {
  if (!isRecord(value)) {
    return false;
  }
  return (
    typeof value.code === "string" &&
    typeof value.message === "string" &&
    typeof value.severity === "string" &&
    IPC_SEVERITIES.includes(value.severity as IpcSeverity) &&
    typeof value.retry === "string" &&
    IPC_RETRIES.includes(value.retry as IpcRetry)
  );
}

export function toFrontendError(value: unknown): FrontendError {
  if (value instanceof FrontendError) {
    return value;
  }
  if (isIpcErrorDto(value)) {
    return new FrontendError(value);
  }
  if (typeof value === "string") {
    return new FrontendError({
      code: "IPC_REQUEST_FAILED",
      message: "The request could not be completed.",
      severity: "error",
      retry: "immediate",
    });
  }
  return new FrontendError({
    code: "APP_UNEXPECTED",
    message: "An unexpected error occurred.",
    severity: "error",
    retry: "never",
  });
}

export function isRetryable(error: FrontendError): boolean {
  return error.retry === "immediate" || error.retry === "afterStateRefresh";
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
