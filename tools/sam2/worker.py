"""Optional local SAM 2.1 inference worker. FilmCraft orchestration remains Rust."""
import sys, os, json, hashlib, tempfile, shutil, traceback
from pathlib import Path
ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / 'source'))
def main():
    import torch
    import numpy as np
    from PIL import Image
    from sam2.build_sam import build_sam2_video_predictor
    header = json.loads(sys.stdin.buffer.readline(65536))
    w, h, n = header['width'], header['height'], header['frameCount']
    objects = header['objects']
    if not (1 <= w <= 320 and 1 <= h <= 320 and 1 <= n and 1 <= len(objects) <= 8 and n * len(objects) <= 100000):
        raise ValueError('SAM request exceeds dimensions or result budget')
    metadata = json.loads((ROOT / 'metadata.json').read_text())
    checkpoint = ROOT / 'sam2.1_hiera_tiny.pt'
    if hashlib.sha256(checkpoint.read_bytes()).hexdigest() != metadata['checkpointSha256']:
        raise ValueError('SAM checkpoint checksum mismatch')
    preference = os.environ.get('FILMCRAFT_SAM2_DEVICE', 'auto')
    if preference not in ('auto','cpu','gpu'): raise ValueError('Unknown SAM device')
    gpu = 'cuda' if torch.cuda.is_available() else 'mps' if torch.backends.mps.is_available() else None
    if preference == 'gpu' and gpu is None: raise ValueError('No CUDA or Apple MPS GPU available')
    device = 'cpu' if preference == 'cpu' else gpu or 'cpu'
    torch.set_num_threads(4)
    predictor = build_sam2_video_predictor('configs/sam2.1/sam2.1_hiera_t.yaml', str(checkpoint), device=device, apply_postprocessing=False)
    tracks=[]; windows=[]
    with tempfile.TemporaryDirectory(prefix='sam-work-',dir=header['frameDir']) as tmp:
        tmp=Path(tmp); frames=tmp/'frames'; frames.mkdir()
        for i in range(n):
            raw=(Path(header['frameDir']) / (str(i)+'.rgba')).read_bytes()
            if len(raw)!=w*h*4: raise ValueError('Invalid rendered frame size')
            Image.fromarray(np.frombuffer(raw,dtype=np.uint8).reshape(h,w,4)[...,:3]).save(frames/f'{i:08}.jpg',quality=100,subsampling=0)
        chunk=tmp/'chunk';chunk.mkdir()
        with torch.inference_mode():
            for obj in objects:
                ref=obj['frameIndex']; points=obj['points']
                if not (0<=ref<n and 1<=len(points)<=64 and any(p['foreground'] for p in points)): raise ValueError('Invalid object prompts')
                masks=[None]*n
                for direction in (1,-1):
                    start=ref; carry=None
                    while True:
                        indices=list(range(start,min(n,start+32))) if direction==1 else list(range(start,max(-1,start-32),-1))
                        if len(indices)==1 and carry is not None:break
                        for f in chunk.iterdir():f.unlink()
                        for local,source in enumerate(indices):shutil.copyfile(frames/f'{source:08}.jpg',chunk/f'{local:08}.jpg')
                        windows.append({'object':obj['name'],'fromFrame':indices[0],'toFrame':indices[-1],'seed':'points' if carry is None else 'carriedMask'})
                        state=predictor.init_state(str(chunk),offload_video_to_cpu=True,offload_state_to_cpu=True)
                        if carry is None:
                            coords=np.asarray([[p['position'][0]*w,p['position'][1]*h] for p in points],dtype=np.float32)
                            labels=np.asarray([int(p['foreground']) for p in points],dtype=np.int32)
                            predictor.add_new_points_or_box(state,frame_idx=0,obj_id=1,points=coords,labels=labels)
                        else: predictor.add_new_mask(state,frame_idx=0,obj_id=1,mask=torch.from_numpy(carry))
                        last=None
                        for local, ids, logits in predictor.propagate_in_video(state):
                            if not torch.isfinite(logits).all():raise ValueError('SAM returned non-finite mask logits')
                            mask=(logits[0,0]>0).to('cpu').numpy()
                            source=indices[local]
                            if masks[source] is None:
                                flat=mask.reshape(-1);edges=np.diff(np.r_[False,flat,False].astype(np.int8));beg=np.flatnonzero(edges==1);end=np.flatnonzero(edges==-1)
                                masks[source]={'runs':[[int(a),int(b-a)] for a,b in zip(beg,end)]}
                            if local==len(indices)-1:last=mask
                        del state
                        end=indices[-1]
                        if end==n-1 or end==0 or last is None or not last.any():break
                        carry=last;start=end
                tracks.append({'object':obj,'masks':masks})
    result={'analyzer':'SAM 2.1','frameCount':n,'maskSize':[w,h],'tracks':tracks,'runtime':dict(metadata,device=device,executionRequest=preference,windowFrames=32,overlapFrames=1,logitThreshold=0.0,frameEncoding='JPEG quality100 subsampling0',postprocessing=False,windows=windows)}
    encoded=json.dumps(result,allow_nan=False,separators=(',',':'))
    if len(encoded)>67108864:raise ValueError('SAM result exceeds 64 MB; choose fewer objects or a shorter range')
    print(encoded,flush=True)
if __name__=='__main__':
    try:main()
    except Exception as e:
        traceback.print_exc(file=sys.stderr);print(json.dumps({'error':str(e)[:2000]}),flush=True);sys.exit(1)
