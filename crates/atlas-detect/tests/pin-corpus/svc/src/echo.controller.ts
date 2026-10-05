import { Controller, Get, Param } from '@nestjs/common';

function relayEcho(id: string) {
  return fetch('/echo/' + id);
}

function relayStatus(path: string) {
  return fetch('/status' + path);
}

@Controller('echo')
export class EchoController {
  @Get(':id')
  echo(@Param('id') id: string) {
    return relayEcho(id);
  }

  @Get('health')
  health() {
    return relayStatus('/health');
  }
}
