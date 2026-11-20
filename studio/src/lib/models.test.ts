import { describe, expect, it } from 'vitest';
import type { AutoAnswer, FitDetail, Model } from '../api/types';
import { autoReason, chosenModel, memoryLabel, switcherOptions, timingLabel } from './models';

const fits: FitDetail = { tier: 'fits', need_mib: 4403, gpu_mib: 8151, usable_ram_mib: 14336 };
const offload: FitDetail = { tier: 'offload', need_mib: 9734, gpu_mib: 8151, usable_ram_mib: 14336 };
const tooBig: FitDetail = { tier: 'needs_more_memory', need_mib: 30000, gpu_mib: 8151, usable_ram_mib: 14336 };

const model = (p: Partial<Model> & { id: string }): Model =>
  ({ name: p.id, tag: null, size_bytes: 1e9, state: 'installed', tier: 'tested', fit: 'fits', fit_detail: fits, tasks: ['generate'], custom: null, ...p }) as Model;

const zimage = model({ id: 'z-image-turbo:q4', name: 'Z-Image Turbo', tiers: { generate: 'tested' } });
const klein = model({ id: 'flux2-klein-4b:q8', name: 'FLUX.2 klein 4B', tasks: ['generate', 'edit'], tiers: { generate: 'tested', edit: 'tested' } });
const longcat = model({ id: 'longcat-image:q4', name: 'LongCat-Image', fit: 'offload', fit_detail: offload, tiers: { generate: 'tested' } });
const untested = model({ id: 'chroma1-hd:q4', name: 'Chroma1-HD', tier: 'untested', tiers: { generate: 'untested' } });
const custom = model({ id: 'hf.co/a/b:c.gguf', name: 'c', tier: 'untested', custom: { source: 'hf.co/a/b:c.gguf' }, label: 'Custom, not reviewed by Dawnroom' });
const pulling = model({ id: 'sd15', name: 'SD 1.5', state: 'pulling' });
const qwenEdit = model({ id: 'qwen-image-edit-2511:q4', name: 'Qwen-Image-Edit 2511', tasks: ['edit'], tiers: { edit: 'tested' }, fit: 'offload', fit_detail: offload });

describe('the model switcher', () => {
  it('lists only installed models that can do the task: library before custom, then fits before offload, then by name', () => {
    const all = [untested, custom, longcat, klein, pulling, zimage, qwenEdit];
    expect(switcherOptions(all, 'generate').map((m) => m.id)).toEqual(['chroma1-hd:q4', 'flux2-klein-4b:q8', 'z-image-turbo:q4', 'longcat-image:q4', 'hf.co/a/b:c.gguf']);
    expect(switcherOptions(all, 'edit').map((m) => m.id)).toEqual(['flux2-klein-4b:q8', 'qwen-image-edit-2511:q4']);
  });

  it('says the time only when we have one for this machine, and never invents one', () => {
    expect(timingLabel({ warm_s: 18.5, cold_s: 26.4, gpu: 'NVIDIA GeForce RTX 5060 Laptop GPU', here: true, label: '' })).toBe('about 19 s');
    expect(timingLabel({ warm_s: 775, cold_s: 750, gpu: 'NVIDIA GeForce RTX 5060 Laptop GPU', here: true, label: '' })).toBe('about 13 min');
    expect(timingLabel({ cold_s: 12, gpu: 'NVIDIA GeForce RTX 4090', here: false, label: '' })).toBe('');
    expect(timingLabel(null)).toBe('');
    expect(timingLabel(undefined)).toBe('');
  });

  it('says where the memory goes: the GPU, RAM offload, an estimate, or too much', () => {
    expect(memoryLabel(fits)).toBe('4.3 GB on the GPU');
    expect(memoryLabel(offload)).toBe('9.5 GB: GPU + 2.0 GB RAM, slower');
    expect(memoryLabel({ ...fits, need_mib: 7270, estimate: true })).toBe('est. 7.1 GB on the GPU');
    expect(memoryLabel(tooBig)).toMatch(/^needs 29\.3 GB, more than this machine has/);
    expect(memoryLabel({ tier: 'no_gpu', need_mib: null, gpu_mib: null, usable_ram_mib: null })).toBe('needs a GPU');
  });

  it("shows Auto's reason in the engine's words, and its way out when it has nothing", () => {
    const auto: AutoAnswer = {
      loaded: null,
      generate: { ok: true, model: 'z-image-turbo:q4', name: 'Z-Image Turbo', reason: 'Z-Image Turbo: the best installed model for making images, best text, about 19 s here', loaded: false },
      edit: { ok: false, code: 'no_model_for_task', error: 'No tested model for editing is installed.', suggest: { model: 'flux2-klein-4b:q8', name: 'FLUX.2 klein 4B', download_bytes: 7.1e9 } },
    };
    expect(autoReason(auto.generate)).toBe('The best installed model for making images, best text, about 19 s here');
    expect(autoReason(auto.edit)).toBe('No tested model for editing is installed.');
    // Auto (no choice) runs Auto's pick; a chosen model wins; nothing when Auto has nothing.
    expect(chosenModel([zimage, klein], auto, 'generate', null)?.id).toBe('z-image-turbo:q4');
    expect(chosenModel([zimage, klein], auto, 'generate', 'flux2-klein-4b:q8')?.id).toBe('flux2-klein-4b:q8');
    expect(chosenModel([zimage, klein], auto, 'edit', null)).toBeUndefined();
  });
});
