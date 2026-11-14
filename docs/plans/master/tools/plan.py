#!/usr/bin/env python3
"""Check the master plan's one list of work and write the documents made from it.

    python3 tools/plan.py            check slices.json; exit 1 on any error
    python3 tools/plan.py --write    check, then rewrite SLICES.md and the generated
                                     parts of BUILD-ORDER.md

slices.json is the only place a slice's build, size and dependencies are written.
BUILD-ORDER.md keeps its prose; the parts between `<!-- gen:NAME -->` and
`<!-- /gen:NAME -->` are replaced. Standard library only.
"""
import collections
import glob
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PLANS = ['P0', 'K1', 'K2', 'K3', 'K4', 'K5', 'T1', 'A1', 'A2', 'M1', 'C1', 'C2', 'W1', 'H1']
ID = r'(?:P0|K[1-5]|T1|A[12]|M1|C[12]|W1|H1)\.\d+[a-z]?'


def load():
    return json.load(open(os.path.join(ROOT, 'slices.json'), encoding='utf-8'))


def id_key(sid):
    plan, n = sid.split('.')
    m = re.match(r'(\d+)(.*)', n)
    return (PLANS.index(plan), int(m.group(1)), m.group(2))


def build_key(bid):
    return (int(bid[0]), bid[1:])


def plan_table_ids(plan):
    """Ids in the slice table at the end of plans/<plan>-*.md (live, retired)."""
    path = glob.glob(os.path.join(ROOT, 'plans', plan + '-*.md'))
    if not path:
        return None, None
    lines = open(path[0], encoding='utf-8').read().split('\n')
    best = []
    for i, line in enumerate(lines):
        if re.match(r'^\|\s*Id\s*\|', line):
            rows = []
            j = i + 2
            while j < len(lines) and lines[j].startswith('|'):
                rows.append(lines[j])
                j += 1
            if len(rows) >= len(best):
                best = rows
    live, retired = set(), set()
    for row in best:
        cells = [c.strip() for c in row.split('|')]
        first, size = cells[1].strip('*~ '), cells[3] if len(cells) > 3 else ''
        m = re.match(r'^(%s) to (%s)$' % (ID, ID), first)
        if m:
            a, b = m.groups()
            for n in range(int(a.split('.')[1]), int(b.split('.')[1]) + 1):
                live.add('%s.%d' % (plan, n))
            continue
        if not re.match(r'^%s$' % ID, first):
            continue
        (retired if size in ('—', '-', '') else live).add(first)
    return live, retired


def check(doc):
    errors, notes = [], []
    slices = {}
    for s in doc['slices']:
        if s['id'] in slices:
            errors.append('duplicate id %s' % s['id'])
        slices[s['id']] = s
    builds = {b['id']: b for b in doc['builds']}
    stages = {st['id'] for st in doc['stages']}
    retired = {r['id'] for r in doc['retired']}

    for b in doc['builds']:
        if b['stage'] not in stages:
            errors.append('build %s names stage %s, which does not exist' % (b['id'], b['stage']))
        if str(b['stage']) != b['id'][0]:
            errors.append('build %s: its id should start with its stage %s' % (b['id'], b['stage']))

    def stage(sid):
        return builds[slices[sid]['build']]['stage']

    for s in doc['slices']:
        sid = s['id']
        if s['plan'] != sid.split('.')[0]:
            errors.append('%s: plan field %s does not match its id' % (sid, s['plan']))
        if s['build'] is None:
            if not s.get('parked'):
                errors.append('%s has no build and no "parked" reason' % sid)
            continue
        if s['build'] not in builds:
            errors.append('%s is in build %s, which does not exist' % (sid, s['build']))
            continue
        for kind in ('needs', 'soft'):
            for d in s[kind]:
                if d in retired:
                    errors.append('%s %s %s, which is retired' % (sid, kind, d))
                elif d not in slices:
                    errors.append('%s %s %s, which is not in the list' % (sid, kind, d))
        for d in s['needs']:
            if d not in slices:
                continue
            if slices[d]['build'] is None:
                errors.append('%s needs %s, which is parked' % (sid, d))
            elif slices[d]['build'] == s['build']:
                pass
            elif stage(d) > stage(sid):
                errors.append('%s (build %s) needs %s, which comes later (build %s)'
                              % (sid, s['build'], d, slices[d]['build']))
            elif stage(d) == stage(sid):
                errors.append('%s (build %s) needs %s from build %s in the same stage: '
                              'move one of them, or make it a soft need with a stopgap'
                              % (sid, s['build'], d, slices[d]['build']))

    # No cycles among hard needs.
    state = {}

    def visit(sid, path):
        if state.get(sid) == 2:
            return
        if state.get(sid) == 1:
            errors.append('cycle: ' + ' > '.join(path[path.index(sid):] + [sid]))
            return
        state[sid] = 1
        for d in slices[sid]['needs']:
            if d in slices:
                visit(d, path + [sid])
        state[sid] = 2

    sys.setrecursionlimit(10000)
    for sid in slices:
        visit(sid, [])

    # The plans' own tables hold the same ids.
    for plan in PLANS:
        live, gone = plan_table_ids(plan)
        if live is None:
            notes.append('plans/%s-*.md not found; its table was not compared' % plan)
            continue
        mine = {sid for sid in slices if sid.split('.')[0] == plan}
        for sid in sorted(live - mine, key=id_key):
            errors.append('%s is in plans/%s but not in slices.json' % (sid, plan))
        for sid in sorted(mine - live, key=id_key):
            errors.append('%s is in slices.json but not in the table of plans/%s' % (sid, plan))
        for sid in sorted(gone - retired, key=id_key):
            errors.append('%s is retired in plans/%s but not listed under "retired"' % (sid, plan))

    for m in doc['milestones']:
        for sid in m['slices']:
            if sid not in slices or slices[sid]['build'] is None:
                errors.append('milestone "%s" names %s, which is not scheduled' % (m['name'][:40], sid))
    for group in ('decisions', 'hot_files'):
        for row in doc.get(group, []):
            for sid in row['slices']:
                if sid not in slices:
                    errors.append('%s row names %s, which is not in the list' % (group, sid))
    return errors, notes, slices, builds


def ordered(build_id, slices):
    """Slices of one build, each after the slices it needs in the same build."""
    mine = [s for s in slices.values() if s['build'] == build_id]
    done, out = set(), []
    left = sorted(mine, key=lambda s: (s.get('seq', 0), id_key(s['id'])))
    while left:
        for s in left:
            if all(d in done or slices.get(d, {}).get('build') != build_id for d in s['needs']):
                out.append(s)
                done.add(s['id'])
                left.remove(s)
                break
        else:
            raise SystemExit('cannot order build %s' % build_id)
    return out


def build_needs(build_id, slices):
    out = set()
    for s in slices.values():
        if s['build'] == build_id:
            for d in s['needs']:
                b = slices[d]['build']
                if b and b != build_id:
                    out.add(b)
    return sorted(out, key=build_key)


def direct_needs(build_id, slices, memo={}):
    """Needed builds, leaving out those already implied by another needed build."""
    def closure(b):
        if b not in memo:
            memo[b] = set()
            for n in build_needs(b, slices):
                memo[b] |= {n} | closure(n)
        return memo[b]
    needs = build_needs(build_id, slices)
    implied = set()
    for n in needs:
        implied |= closure(n)
    return [n for n in needs if n not in implied]


def gen(doc, slices, builds):
    out = {}
    live = [s for s in doc['slices'] if s['build'] is not None]
    parked = [s for s in doc['slices'] if s['build'] is None]
    by_plan = collections.Counter(s['plan'] for s in doc['slices'])
    by_stage = collections.Counter(builds[s['build']]['stage'] for s in live)
    nb = collections.Counter(b['stage'] for b in doc['builds'])

    c = []
    c.append('- **%d slices** in the fourteen plans (%s). %d are in builds; %d are parked '
             'with a reason (%s). %d more ids are retired and kept only as pointers (%s).'
             % (len(doc['slices']), ', '.join('%s %d' % (p, by_plan[p]) for p in PLANS),
                len(live), len(parked), ', '.join(s['id'] for s in parked),
                len(doc['retired']), ', '.join(r['id'] for r in doc['retired'])))
    c.append('- **%d builds in %d stages.** One build is one long session, one branch and one '
             'pull request.' % (len(doc['builds']), len(doc['stages'])))
    c.append('')
    c.append('| Stage | Name | Builds | Slices |')
    c.append('| --- | --- | --- | --- |')
    for st in doc['stages']:
        c.append('| %d | %s | %d | %d |' % (st['id'], st['name'], nb[st['id']], by_stage[st['id']]))
    c.append('| | **Total** | **%d** | **%d** |' % (len(doc['builds']), len(live)))
    out['counts'] = '\n'.join(c)

    t = []
    for st in doc['stages']:
        t.append('### Stage %d: %s' % (st['id'], st['name']))
        t.append('')
        t.append('**Done when:** ' + st['gate'])
        t.append('')
        for b in sorted([b for b in doc['builds'] if b['stage'] == st['id']], key=lambda b: build_key(b['id'])):
            sl = ordered(b['id'], slices)
            sizes = collections.Counter(s['size'] for s in sl)
            size_txt = ', '.join('%d %s' % (sizes[k], k) for k in ('S', 'S/M', 'M', 'L') if sizes[k])
            plans = sorted({s['plan'] for s in sl}, key=PLANS.index)
            t.append('#### Build %s: %s' % (b['id'], b['name']))
            t.append('')
            t.append(b['delivers'])
            t.append('')
            needs = direct_needs(b['id'], slices)
            t.append('- **Slices:** %d (%s), from %s.' % (len(sl), size_txt, ', '.join(plans)))
            if b.get('starts_after_note'):
                t.append('- **Starts after:** %s.' % b['starts_after_note'])
            else:
                t.append('- **Starts after:** %s.' % (', '.join('build ' + n for n in needs) if needs else 'nothing; it can start now'))
            soft = []
            for s in sl:
                for d in s['soft']:
                    db = slices[d]['build']
                    if db and db != b['id'] and builds[db]['stage'] >= b['stage']:
                        soft.append('%s on %s (build %s)' % (s['id'], d, db))
            if soft:
                t.append('- **Stopgap unless it has landed:** %s.' % '; '.join(soft))
            t.append('- **In this order:** ' + '; '.join('%s %s' % (s['id'], s['name']) for s in sl) + '.')
            t.append('')
    out['stages'] = '\n'.join(t).rstrip()

    m = ['| After stage | What a person notices | The slices that make it |', '| --- | --- | --- |']
    rows = []
    for ms in doc['milestones']:
        stg = max(builds[slices[s]['build']]['stage'] for s in ms['slices'])
        rows.append((stg, ms))
    for stg, ms in sorted(rows, key=lambda r: r[0]):
        m.append('| %d | %s | %s |' % (stg, ms['name'], ', '.join(ms['slices'])))
    out['milestones'] = '\n'.join(m)

    d = ['| Build | Question | Slices | Recommendation |', '| --- | --- | --- | --- |']
    rows = []
    for row in doc.get('decisions', []):
        bs = sorted({slices[s]['build'] for s in row['slices'] if slices[s]['build']}, key=build_key)
        rows.append((build_key(bs[0]) if bs else (9, ''), bs[0] if bs else 'none', row))
    for _, b, row in sorted(rows, key=lambda r: r[0]):
        d.append('| %s | %s | %s | %s |' % (b, row['question'], row['slices_text'], row['recommendation']))
    out['decisions'] = '\n'.join(d)

    h = ['| Stage | File | Builds that change it | Slices |', '| --- | --- | --- | --- |']
    merged = collections.OrderedDict()
    for row in doc.get('hot_files', []):
        for f in row['files']:
            for s in row['slices']:
                b = slices[s]['build']
                if b:
                    merged.setdefault((builds[b]['stage'], f), collections.OrderedDict()).setdefault(b, []).append(s)
    for (stg, f), bs in sorted(merged.items(), key=lambda kv: (kv[0][0], kv[0][1])):
        if len(bs) < 2:
            continue
        h.append('| %d | `%s` | %s | %s |' % (stg, f, ', '.join(sorted(bs, key=build_key)),
                                              ', '.join(sorted({s for v in bs.values() for s in v}, key=id_key))))
    out['hot'] = '\n'.join(h)
    return out


def slices_md(doc, slices):
    live = [s for s in doc['slices'] if s['build'] is not None]
    o = ['# Every slice, by plan', '',
         'Generated from `slices.json` by `tools/plan.py --write`. Do not edit by hand.', '',
         '%d slices; %d in builds, %d parked; %d retired ids.'
         % (len(doc['slices']), len(live), len(doc['slices']) - len(live), len(doc['retired'])), '']
    for plan in PLANS:
        rows = sorted([s for s in doc['slices'] if s['plan'] == plan], key=lambda s: id_key(s['id']))
        o += ['## %s (%d)' % (plan, len(rows)), '',
              '| Id | Name | Size | Build | Needs | Soft needs (stopgap) |', '| --- | --- | --- | --- | --- | --- |']
        for s in rows:
            o.append('| %s | %s | %s | %s | %s | %s |' % (
                s['id'], s['name'], s['size'], s['build'] or 'parked: ' + s['parked'],
                ', '.join(s['needs']) or 'none', ', '.join(s['soft']) or ''))
        o.append('')
    o += ['## Retired ids', '']
    o += ['- %s: %s' % (r['id'], r['name']) for r in doc['retired']]
    return '\n'.join(o) + '\n'


def main():
    doc = load()
    errors, notes, slices, builds = check(doc)
    for n in notes:
        print('note:', n)
    if errors:
        for e in errors:
            print('ERROR:', e)
        print('%d errors' % len(errors))
        return 1
    live = [s for s in doc['slices'] if s['build'] is not None]
    print('ok: %d slices (%d in %d builds, %d parked), %d stages, no cycles, '
          'every hard need lands in an earlier stage or the same build'
          % (len(doc['slices']), len(live), len(doc['builds']), len(doc['slices']) - len(live), len(doc['stages'])))
    if '--write' in sys.argv:
        parts = gen(doc, slices, builds)
        path = os.path.join(ROOT, 'BUILD-ORDER.md')
        text = open(path, encoding='utf-8').read()
        for name, body in parts.items():
            pat = re.compile(r'(<!-- gen:%s -->\n).*?(<!-- /gen:%s -->)' % (name, name), re.S)
            if not pat.search(text):
                print('ERROR: BUILD-ORDER.md has no gen:%s section' % name)
                return 1
            text = pat.sub(lambda mo: mo.group(1) + body + '\n' + mo.group(2), text)
        open(path, 'w', encoding='utf-8').write(text)
        open(os.path.join(ROOT, 'SLICES.md'), 'w', encoding='utf-8').write(slices_md(doc, slices))
        print('wrote BUILD-ORDER.md (generated parts) and SLICES.md')
    return 0


if __name__ == '__main__':
    sys.exit(main())
