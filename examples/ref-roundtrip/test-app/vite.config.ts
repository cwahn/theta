import { defineConfig } from 'vite'
import theta from 'vite-plugin-theta'

export default defineConfig({
  plugins: [...theta({ crates: ['../ref-actors'] })],
})
