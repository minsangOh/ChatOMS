import { isRetryable, type FrontendError } from "../ipc/errors";

interface ErrorStateProps {
  error: FrontendError;
  onRetry?: () => void;
}

export function ErrorState({ error, onRetry }: ErrorStateProps) {
  const showRetry = onRetry !== undefined && isRetryable(error);
  const actionLabel = error.retry === "afterStateRefresh" ? "Refresh" : "Retry";

  return (
    <section className="state-panel state-panel--error" role="alert" aria-labelledby="error-title">
      <p className="state-kicker">{error.severity}</p>
      <h2 id="error-title">Unable to load this information</h2>
      <p>{error.message}</p>
      <p className="error-code">Error code: {error.code}</p>
      {showRetry ? (
        <button className="button" type="button" onClick={onRetry}>
          {actionLabel}
        </button>
      ) : null}
    </section>
  );
}
