"""Install optional pinned SAM inference assets outside the FilmCraft repository."""
import argparse, concurrent.futures, hashlib, json, shlex, shutil, subprocess, urllib.request
from pathlib import Path
REV = '2b90b9f5ceec907a1c18123530e92e794ad901a4'
SHA = '7402e0d864fa82708a20fbd15bc84245c2f26dff0eb43a4b5b93452deb34be69'
URL = 'https://huggingface.co/facebook/sam2.1-hiera-tiny/resolve/main/sam2.1_hiera_tiny.pt'
def main():
    parser=argparse.ArgumentParser();parser.add_argument('--python',required=True,help='Original Python executable in your PyTorch virtual environment');parser.add_argument('--root',required=True);args=parser.parse_args()
    root=Path(args.root).expanduser().resolve();root.mkdir(parents=True,exist_ok=True)
    tree=json.load(urllib.request.urlopen(f'https://api.github.com/repos/facebookresearch/sam2/git/trees/{REV}?recursive=1',timeout=30))
    files=[f for f in tree['tree'] if f['type']=='blob' and (f['path'].startswith('sam2/') and f['path'].endswith(('.py','.yaml')) or f['path'] in ('LICENSE','README.md'))]
    def get(item):
        name=item['path'];data=urllib.request.urlopen(f'https://raw.githubusercontent.com/facebookresearch/sam2/{REV}/{name}',timeout=60).read()
        if hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest()!=item['sha']:raise ValueError('Source checksum mismatch: '+name)
        dest=root/'source'/name;dest.parent.mkdir(parents=True,exist_ok=True);dest.write_bytes(data)
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:list(pool.map(get,files))
    checkpoint=root/'sam2.1_hiera_tiny.pt'
    if not checkpoint.exists() or hashlib.sha256(checkpoint.read_bytes()).hexdigest()!=SHA:
        pending=root/'checkpoint.download'
        with urllib.request.urlopen(URL,timeout=60) as resp,pending.open('wb') as out:
            shutil.copyfileobj(resp,out)
        if hashlib.sha256(pending.read_bytes()).hexdigest()!=SHA:raise ValueError('Checkpoint checksum mismatch')
        pending.replace(checkpoint)
    python=str(Path(args.python).expanduser().absolute())
    subprocess.run([python,'-c','import torch, torchvision; from packaging.version import Version; assert Version(torch.__version__.split("+")[0]) >= Version("2.5.1") and Version(torchvision.__version__.split("+")[0]) >= Version("0.20.1")'],check=True)
    subprocess.run([python,'-m','pip','install','hydra-core==1.3.2','iopath==0.1.10','tqdm==4.67.1'],check=True)
    shutil.copyfile(Path(__file__).with_name('worker.py'),root/'worker.py')
    launcher=root/'python';launcher.write_text('#!/bin/sh\nexec '+shlex.quote(python)+' "$@"\n');launcher.chmod(0o755)
    (root/'metadata.json').write_text(json.dumps(dict(sourceRevision=REV,checkpointSha256=SHA,checkpointUrl=URL,model='sam2.1_hiera_tiny',license='Apache-2.0')))
    print('Installed SAM 2.1 Tiny at',root)
if __name__=='__main__':main()
