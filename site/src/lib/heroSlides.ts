/**
 * The hero's prints, in order: the developing RealVisXL print, then one square sample from each other
 * model. Shared by the print (HeroReel) and the contact strip beside the headline (HeroStrip).
 */
import hero from '../hero.json';
import samples from '../samples.json';
import { loadModels, type Sample } from './registry';
import { stockOf } from './stocks';

export interface Slide { s: Sample; name: string; preset: string; speed: string; thumb: string }

export function heroSlides(): Slide[] {
  const models = loadModels();
  const rest: [string, number][] = [['z-image-turbo', 51], ['flux2-klein-4b', 20]];
  const list = [hero as unknown as Sample, ...rest.map(([m, f]) => (samples as Sample[]).find((s) => s.model === m && s.frame === f)!)];
  return list.map((s, i) => {
    const m = models.find((x) => x.id === s.model)!;
    const meas = m.measured?.[0];
    const speed = meas ? (meas.warm_s ? `about ${Math.round(meas.warm_s)} s an image` : `about ${Math.round(meas.cold_s)} s an image (first run)`) : '';
    // The developing print has a 640 px copy; the samples have contact-sheet thumbnails.
    const thumb = i === 0 ? s.file.replace('.webp', '-640.webp') : s.file.replace('/samples/', '/samples/thumb/');
    return { s, name: m.name, preset: stockOf(m.id).band.split(' · ')[0], speed, thumb };
  });
}
