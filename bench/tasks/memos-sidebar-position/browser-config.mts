import { mergeConfig } from 'vitest/config';
import { playwright } from '@vitest/browser-playwright';
import base from './vitest.config.mts';

export default mergeConfig(base, {
  test: {
    browser: { enabled: true, headless: true, provider: playwright(), instances: [{ browser: 'chromium' }] },
  },
});
