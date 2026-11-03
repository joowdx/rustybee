import { HttpException, HttpStatus, Injectable, Logger } from '@nestjs/common'
import { InjectModel } from '@nestjs/mongoose'
import { Model } from 'mongoose'
import { Device, DeviceDocument } from './schemas/device.schema'
import { heartbeatAndroidConfig } from './fcm-push-options'
import { isPermanentFcmTokenError } from './fcm-token-error'
import { FcmTransport } from './transport/device-transport'

// One manual wake per device per 30 seconds
export const WAKE_COOLDOWN_MS = 30 * 1000

// Sends a heartbeat_check on demand so the device reports in right away.
// FCM only: a high-priority push gets through Doze, while a dozing phone can
// still look MQTT-online until its keepalive times out.
@Injectable()
export class DeviceWakeService {
  private readonly logger = new Logger(DeviceWakeService.name)
  private readonly lastWakeAt = new Map<string, number>()

  constructor(
    @InjectModel(Device.name) private deviceModel: Model<DeviceDocument>,
    private readonly fcm: FcmTransport,
  ) {}

  async wake(deviceId: string): Promise<{ sent: true }> {
    const device = await this.deviceModel.findById(deviceId)

    if (!device) {
      throw new HttpException(
        { error: 'Device not found' },
        HttpStatus.NOT_FOUND,
      )
    }

    if (!device.enabled) {
      throw new HttpException(
        { error: 'Device is disabled' },
        HttpStatus.BAD_REQUEST,
      )
    }

    if (!device.fcmToken || device.fcmTokenInvalidatedAt) {
      throw new HttpException(
        {
          error:
            'Device has no valid push token. Open the app on the phone to refresh it.',
        },
        HttpStatus.CONFLICT,
      )
    }

    const now = Date.now()
    const last = this.lastWakeAt.get(deviceId)
    if (last !== undefined && now - last < WAKE_COOLDOWN_MS) {
      const seconds = Math.ceil((WAKE_COOLDOWN_MS - (now - last)) / 1000)
      throw new HttpException(
        { error: `Device was just woken. Try again in ${seconds}s.` },
        HttpStatus.TOO_MANY_REQUESTS,
      )
    }
    this.lastWakeAt.set(deviceId, now)

    const response = await this.fcm.sendEach([
      {
        device,
        message: {
          data: { type: 'heartbeat_check' },
          token: device.fcmToken,
          android: heartbeatAndroidConfig(),
        },
      },
    ])

    const result = response.responses[0]
    if (!result?.success) {
      const errorMessage = result?.error?.message || 'Unknown error'
      const errorCode = result?.error?.code || 'UNKNOWN_ERROR'
      this.logger.error(`Failed to wake device ${deviceId}: ${errorMessage}`)

      if (isPermanentFcmTokenError(result?.error)) {
        await this.deviceModel.updateOne(
          { _id: device._id },
          {
            $set: {
              fcmTokenInvalidatedAt: new Date(),
              fcmTokenInvalidReason: `${errorCode}: ${errorMessage}`,
            },
          },
        )
      }

      throw new HttpException(
        { error: 'Failed to send the wake push' },
        HttpStatus.BAD_GATEWAY,
      )
    }

    return { sent: true }
  }
}
