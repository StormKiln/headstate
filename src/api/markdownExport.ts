import { call } from './transport';
export const MARKDOWN_EXPORT_BYTES = 8 * 1024 * 1024;
export type ExportOutcome = 'saved' | 'cancelled' | 'shared' | 'presented';
export function saveMarkdown(markdown: string): Promise<ExportOutcome> {
  if (new TextEncoder().encode(markdown).byteLength > MARKDOWN_EXPORT_BYTES) {
    return Promise.reject(new Error('The export exceeds 8 MiB. Select fewer turns and try again.'));
  }
  return call<ExportOutcome>('save_markdown', {markdown});
}
