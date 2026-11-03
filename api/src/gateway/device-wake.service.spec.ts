import { DeviceWakeService, WAKE_COOLDOWN_MS } from './device-wake.service'

jest.mock('firebase-admin', () => ({
  messaging: jest.fn().mockReturnValue({ sendEach: jest.fn() }),
}))

describe('DeviceWakeService', () => {
  const deviceId = '64b7f0c2a1b2c3d4e5f60718'

  let device: any
  let deviceModel: { findById: jest.Mock; updateOne: jest.Mock }
  let fcm: { sendEach: jest.Mock }
  let service: DeviceWakeService

  beforeEach(() => {
    device = {
      _id: deviceId,
      enabled: true,
      fcmToken: 'fcm-token',
      mqttConnected: true,
      mqttStatusAt: new Date(),
    }
    deviceModel = {
      findById: jest.fn().mockImplementation(async () => device),
      updateOne: jest.fn().mockResolvedValue({}),
    }
    fcm = {
      sendEach: jest.fn().mockResolvedValue({
        responses: [{ success: true }],
        successCount: 1,
        failureCount: 0,
      }),
    }
    service = new DeviceWakeService(deviceModel as any, fcm as any)
  })

  afterEach(() => {
    jest.useRealTimers()
  })

  it('sends a heartbeat_check over FCM even when the device is MQTT-online', async () => {
    await expect(service.wake(deviceId)).resolves.toEqual({ sent: true })

    expect(fcm.sendEach).toHaveBeenCalledTimes(1)
    const [[delivery]] = fcm.sendEach.mock.calls[0]
    expect(delivery.message).toMatchObject({
      data: { type: 'heartbeat_check' },
      token: 'fcm-token',
      android: { priority: 'high', collapseKey: 'heartbeat_check' },
    })
  })

  it('returns 404 for a missing device', async () => {
    device = null
    await expect(service.wake(deviceId)).rejects.toMatchObject({ status: 404 })
    expect(fcm.sendEach).not.toHaveBeenCalled()
  })

  it('returns 400 for a disabled device', async () => {
    device.enabled = false
    await expect(service.wake(deviceId)).rejects.toMatchObject({ status: 400 })
    expect(fcm.sendEach).not.toHaveBeenCalled()
  })

  it('returns 409 without a push token', async () => {
    device.fcmToken = undefined
    await expect(service.wake(deviceId)).rejects.toMatchObject({ status: 409 })
    expect(fcm.sendEach).not.toHaveBeenCalled()
  })

  it('returns 409 when the push token was invalidated', async () => {
    device.fcmTokenInvalidatedAt = new Date()
    await expect(service.wake(deviceId)).rejects.toMatchObject({ status: 409 })
    expect(fcm.sendEach).not.toHaveBeenCalled()
  })

  it('returns 429 inside the cooldown and allows a wake after it', async () => {
    jest.useFakeTimers({ now: new Date('2026-11-03T00:00:00Z') })

    await service.wake(deviceId)
    await expect(service.wake(deviceId)).rejects.toMatchObject({ status: 429 })

    jest.setSystemTime(Date.now() + WAKE_COOLDOWN_MS)
    await expect(service.wake(deviceId)).resolves.toEqual({ sent: true })
    expect(fcm.sendEach).toHaveBeenCalledTimes(2)
  })

  it('invalidates the token on a permanent FCM error and returns 502', async () => {
    fcm.sendEach.mockResolvedValue({
      responses: [
        {
          success: false,
          error: {
            code: 'messaging/registration-token-not-registered',
            message: 'Requested entity was not found.',
          },
        },
      ],
      successCount: 0,
      failureCount: 1,
    })

    await expect(service.wake(deviceId)).rejects.toMatchObject({ status: 502 })
    expect(deviceModel.updateOne).toHaveBeenCalledWith(
      { _id: deviceId },
      {
        $set: expect.objectContaining({
          fcmTokenInvalidatedAt: expect.any(Date),
          fcmTokenInvalidReason: expect.stringContaining(
            'registration-token-not-registered',
          ),
        }),
      },
    )
  })

  it('keeps the token on a transient FCM error', async () => {
    fcm.sendEach.mockResolvedValue({
      responses: [
        {
          success: false,
          error: { code: 'messaging/internal-error', message: 'boom' },
        },
      ],
      successCount: 0,
      failureCount: 1,
    })

    await expect(service.wake(deviceId)).rejects.toMatchObject({ status: 502 })
    expect(deviceModel.updateOne).not.toHaveBeenCalled()
  })
})
