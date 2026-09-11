import json

d = json.load(open(r'C:\Users\Mac\Works\translator-game\tmp_pr.json'))
print('title:', d.get('title'))
print('merged_at:', d.get('merged_at'))
print('merge_commit:', (d.get('merge_commit_sha') or '')[:12])
print('body:', (d.get('body') or '')[:1500])
