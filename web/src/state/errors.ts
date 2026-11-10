import { ApiError, NetworkError } from "../api/client";

/** A specific, actionable sentence for a failed request. */
export function describeError(error: unknown): string {
  if (error instanceof NetworkError) {
    return "The memory server could not be reached. Check that it is running, then try again.";
  }
  if (error instanceof ApiError) {
    switch (error.status) {
      case 401:
        return "Your key is no longer accepted. Sign out and sign in again.";
      case 403:
        return "This key is not allowed to do that.";
      case 404:
        return "That memory no longer exists. It may have been forgotten elsewhere.";
      case 408:
        return "The server took too long. The AI model may be slow; try again.";
      case 429:
        return "Too many requests. Wait a minute, then try again.";
      default:
        return error.message;
    }
  }
  return "Something went wrong. Try again.";
}
