import pathlib, subprocess, hashlib, json
roots = [pathlib.Path('/home/lilith/tmp/svt-tracking/research-dims8-fixed/artifacts')]
rows = []
for root in roots:
    for entry in (root/'index.tsv').read_text().splitlines():
        directory, case = entry.split('\t', 1)
        d = pathlib.Path(directory)
        settings = dict(line.split('=', 1) for line in (d/'settings.txt').read_text().splitlines())
        bd = int(settings['bit_depth'])
        for backend in ['rs', 'c']:
            with (d/(backend+'-decode.log')).open('wb') as log:
                subprocess.run(['aomdec', '--rawvideo', '--output-bit-depth='+str(bd), '-o', str(d/(backend+'-decode.yuv')), str(d/(backend+'.obu'))], stdout=log, stderr=log, check=True)
        a = (d/'rs-decode.yuv').read_bytes()
        b = (d/'c-decode.yuv').read_bytes()
        w,h = int(settings['width']),int(settings['height'])
        assert len(a)==(w*h+2*((w+1)//2)*((h+1)//2))*(2 if bd==10 else 1)
        assert a==b, (case, bd)
        rows.append({'case':case, 'bit_depth':bd, 'decoded_bytes':len(a), 'decoded_sha256':hashlib.sha256(a).hexdigest(), 'directory':directory})
pathlib.Path('/home/lilith/tmp/svt-tracking/research-dims8-independent-decode.json').write_text(json.dumps(rows, indent=2)+'\n')
print('Independent C/Rust decode pairs identical:',len(rows))
