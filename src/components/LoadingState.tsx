interface LoadingStateProps {
  message?: string;
}

export function LoadingState({ message = "Loading…" }: LoadingStateProps) {
  return (
    <div className="state-panel state-panel--loading" role="status" aria-live="polite">
      <span className="loading-mark" aria-hidden="true" />
      <p>{message}</p>
    </div>
  );
}
