import { HttpException, HttpStatus } from '@nestjs/common'
import { GatewayController } from './gateway.controller'

jest.mock('firebase-admin', () => ({
  messaging: jest.fn().mockReturnValue({ sendEach: jest.fn() }),
}))

describe('GatewayController mqtt-credentials', () => {
  const deviceId = '64b7f0c2a1b2c3d4e5f60718'

  it('wraps the new credentials in data', async () => {
    const credentials = {
      url: 'wss://textbee-mqtt.example.com/mqtt',
      username: `device-${deviceId}`,
      password: 'secret',
      topicPrefix: `textbee/devices/${deviceId}/`,
    }
    const mqttService = {
      rotateDeviceCredentials: jest.fn().mockResolvedValue(credentials),
    }
    const controller = new GatewayController({} as any, mqttService as any, {} as any)

    await expect(controller.rotateMqttCredentials(deviceId)).resolves.toEqual({
      data: credentials,
    })
    expect(mqttService.rotateDeviceCredentials).toHaveBeenCalledWith(deviceId)
  })

  it('passes the disabled error through', async () => {
    const mqttService = {
      rotateDeviceCredentials: jest
        .fn()
        .mockRejectedValue(
          new HttpException({ error: 'MQTT is not enabled on this server' }, HttpStatus.CONFLICT),
        ),
    }
    const controller = new GatewayController({} as any, mqttService as any, {} as any)

    await expect(controller.rotateMqttCredentials(deviceId)).rejects.toMatchObject({
      status: 409,
    })
  })
})
