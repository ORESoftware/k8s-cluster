import { initTelemetry } from '@dd/telemetry';

const telemetry = initTelemetry('dd-web-scraper-supervisor');
let telemetryClosing = false;

async function shutdownTelemetry(): Promise<void> {
  if (telemetryClosing) return;
  telemetryClosing = true;
  try {
    await telemetry.shutdown();
  } catch (error) {
    console.error(
      JSON.stringify({
        timestamp: new Date().toISOString(),
        level: 'error',
        service: 'dd-web-scraper-supervisor',
        event: 'telemetry_shutdown_failed',
        error: error instanceof Error ? error.message : String(error),
      }),
    );
  }
}

process.once('SIGTERM', () => void shutdownTelemetry());
process.once('SIGINT', () => void shutdownTelemetry());
process.once('beforeExit', () => void shutdownTelemetry());

try {
  await import('./scraper-supervisor.js');
} catch (error) {
  await shutdownTelemetry();
  throw error;
}
