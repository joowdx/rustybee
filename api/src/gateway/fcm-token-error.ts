// True when FCM says the token will never work again (app uninstalled or the
// token rotated), as opposed to a transient send failure.
export function isPermanentFcmTokenError(
  error: { code?: string; message?: string } | null | undefined,
): boolean {
  if (!error) {
    return false
  }

  const normalizedCode = String(error.code || '')
    .toLowerCase()
    .replace(/^messaging\//, '')
  const normalizedMessage = String(error.message || '').toLowerCase()

  if (
    normalizedCode === 'registration-token-not-registered' ||
    normalizedCode === 'unregistered' ||
    normalizedCode === 'invalid-registration-token'
  ) {
    return true
  }

  if (
    normalizedMessage.includes('requested entity was not found') ||
    normalizedMessage.includes('not registered') ||
    normalizedMessage.includes('registration token is not a valid')
  ) {
    return true
  }

  return false
}
