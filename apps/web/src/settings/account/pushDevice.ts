import { addDevice, getPushConfig, type PushDevice } from './usageApi'

// Turning on push for this browser (N3): ask permission, subscribe with the
// workspace's public key, and hand the endpoint to the server, which keeps it
// encrypted. The endpoint is never shown back; the server keeps only its
// hash in the clear, which is how this browser finds its own row.

export type PushSupport = 'supported' | 'unsupported' | 'insecure'

export function pushSupport(): PushSupport {
  if (typeof window === 'undefined') return 'unsupported'
  if (!window.isSecureContext) return 'insecure'
  if (!('serviceWorker' in navigator) || !('PushManager' in window) || !('Notification' in window)) return 'unsupported'
  return 'supported'
}

function base64UrlToBytes(value: string) {
  const padded = value.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(value.length / 4) * 4, '=')
  const binary = atob(padded)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index)
  return bytes
}

function bytesToBase64Url(buffer: ArrayBuffer | null) {
  if (!buffer) return ''
  let binary = ''
  for (const byte of new Uint8Array(buffer)) binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

export async function endpointHash(endpoint: string) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(endpoint))
  return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('')
}

/** The hash of this browser's current push endpoint, if it has one. */
export async function currentEndpointHash() {
  if (pushSupport() !== 'supported') return null
  const registration = await navigator.serviceWorker.getRegistration()
  const subscription = await registration?.pushManager.getSubscription()
  return subscription ? endpointHash(subscription.endpoint) : null
}

/** A short name for this device, such as "Chrome on Android". */
export function deviceLabel(userAgent = navigator.userAgent) {
  const browser = /Edg\//.test(userAgent)
    ? 'Edge'
    : /Firefox\//.test(userAgent)
      ? 'Firefox'
      : /Chrome\//.test(userAgent)
        ? 'Chrome'
        : /Safari\//.test(userAgent)
          ? 'Safari'
          : 'Browser'
  const system = /Android/.test(userAgent)
    ? 'Android'
    : /iPhone|iPad/.test(userAgent)
      ? 'iOS'
      : /Mac OS X/.test(userAgent)
        ? 'macOS'
        : /Windows/.test(userAgent)
          ? 'Windows'
          : /Linux/.test(userAgent)
            ? 'Linux'
            : null
  return system ? `${browser} on ${system}` : browser
}

export class PushPermissionDenied extends Error {}

export async function enablePushHere(): Promise<PushDevice> {
  const permission = await Notification.requestPermission()
  if (permission !== 'granted') throw new PushPermissionDenied('Notifications are blocked for this site.')
  const registration = (await navigator.serviceWorker.getRegistration()) ?? (await navigator.serviceWorker.register('/sw.js'))
  await navigator.serviceWorker.ready
  const config = await getPushConfig()
  const key = base64UrlToBytes(config.publicKey)
  let subscription = await registration.pushManager.getSubscription()
  // A subscription made with another workspace's key can't receive this one's pushes.
  const existingKey = subscription?.options.applicationServerKey
  if (subscription && existingKey && bytesToBase64Url(existingKey) !== config.publicKey) {
    await subscription.unsubscribe()
    subscription = null
  }
  subscription ??= await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key })
  return addDevice({
    endpoint: subscription.endpoint,
    keys: {
      p256dh: bytesToBase64Url(subscription.getKey('p256dh')),
      auth: bytesToBase64Url(subscription.getKey('auth')),
    },
    organizationId: config.organizationId,
    label: deviceLabel(),
  })
}

/** Stop this browser's subscription after its row is removed. */
export async function unsubscribeHere() {
  if (pushSupport() !== 'supported') return
  const registration = await navigator.serviceWorker.getRegistration()
  const subscription = await registration?.pushManager.getSubscription()
  await subscription?.unsubscribe()
}
