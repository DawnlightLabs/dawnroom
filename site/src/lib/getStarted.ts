/**
 * Install → get a model → first image, for Studio and the CLI. One source for the landing's
 * "Get started" and /docs Install, so the two can't drift. Sizes come from the registry.
 */
import { loadModels, gb } from './registry';
import { BIN, DOWNLOAD_URL } from '../config';

export const FIRST_PROMPT = 'a lighthouse at dawn';

export interface Step { title: string; text: string; cmd?: string; link?: { href: string; label: string } }

export function getStarted(): { studio: Step[]; cli: Step[] } {
  const models = loadModels();
  const size = (id: string) => gb(models.find((m) => m.id === id)?.bytes ?? 0);
  const quality = size('z-image-turbo');
  const fast = size('flux2-klein-4b');
  const ready = !!DOWNLOAD_URL;

  return {
    studio: [
      {
        title: 'Install',
        text: ready ? 'Run the Windows installer.' : 'The Windows installer is coming with v0.1.',
        link: { href: '/download', label: 'Download page' },
      },
      { title: 'Get a model', text: `First run checks your graphics card and offers a starter: Quality (${quality}) or Fast (${fast}). It downloads once, then works offline.` },
      { title: 'First image', text: 'Type a prompt and press Develop. You can write it while the model downloads.' },
    ],
    cli: [
      {
        title: 'Install',
        text: `${ready ? `The installer puts ${BIN} on your PATH.` : `${BIN} comes with the v0.1 installer; today, build it from source.`} Then start the engine and leave it running.`,
        cmd: `${BIN} serve`,
        link: ready ? undefined : { href: '/docs#from-source', label: 'Build from source' },
      },
      { title: 'Get a model', text: `The Quality model, ${quality}. Resumes if it stops; every file is checked.`, cmd: `${BIN} pull z-image-turbo` },
      { title: 'First image', text: 'It prints the path of the PNG when it’s ready. Add -o FILE to copy it somewhere.', cmd: `${BIN} run "${FIRST_PROMPT}"` },
    ],
  };
}
