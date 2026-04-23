import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['src/plugins/__tests__/**/*.test.ts'],
  },
});
