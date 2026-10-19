#!/usr/bin/env python3
"""Compare per-user p99s or observed maxima without pooling packets or hiding schedule delay."""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path


def load(path, histogram_bounds=False, statistic="p99"):
    with path.open() as stream:
        rows = list(csv.DictReader(stream))
    if not rows:
        raise ValueError(f'{path}: no user samples')
    ids = set()
    values = {'send_to_receive': [], 'intended_send_to_receive': []}
    samples = []
    maxima = {key: 0.0 for key in values}
    selected = 'max' if statistic == 'max' else ('p99_upper' if histogram_bounds else 'p99')
    unit = 'ms' if f'{selected}_ms' in rows[0] else 'ns'
    scale = 1 if unit == 'ms' else 1e6
    actual_column = f'{selected}_{unit}'
    intended_column = f'schedule_{selected}_{unit}'
    for row in rows:
        raw_identity = row['id']
        if not raw_identity or not raw_identity.isascii() or not raw_identity.isdecimal():
            raise ValueError(f'{path}: invalid user ID; expected decimal uint64 session ID')
        identity = int(raw_identity)
        if not 0 < identity < 2**64 - 1:
            raise ValueError(f'{path}: invalid user ID; reserved or outside uint64 session range')
        if identity in ids:
            raise ValueError(f'{path}: duplicate user ID')
        ids.add(identity)
        count = int(row['samples'])
        if count <= 0:
            raise ValueError(f'{path}: nonpositive sample count')
        samples.append(count)
        for key, column, maximum in [('send_to_receive', actual_column, f'max_{unit}'),
                                     ('intended_send_to_receive', intended_column, f'schedule_max_{unit}')]:
            observed, top = float(row[column]) / scale, float(row[maximum]) / scale
            if not all(math.isfinite(v) and v >= 0 for v in [observed, top]) or (not histogram_bounds and observed > top):
                raise ValueError(f'{path}: invalid latency values')
            # ceil(0.99 * n) selects the maximum for 1..99 observations.
            if statistic == 'p99' and count < 100:
                inconsistent = observed < top if histogram_bounds else observed != top
                if inconsistent:
                    raise ValueError(f'{path}: nearest-rank p99 must select the maximum with fewer than 100 samples')
            values[key].append(observed)
            maxima[key] = max(maxima[key], top)
        if (values['intended_send_to_receive'][-1] < values['send_to_receive'][-1]
                or float(row[f'schedule_max_{unit}']) < float(row[f'max_{unit}'])):
            raise ValueError(f'{path}: intended-send latency precedes actual-send latency')
    return {key: sorted(v) for key, v in values.items()}, samples, maxima


def nearest(values, percent):
    """Nearest rank, consistent with the capacity client's per-user statistics."""
    return values[(len(values) * percent + 99) // 100 - 1]


def summarize(values, samples, maxima, thresholds, histogram_bounds=False, statistic="p99"):
    metric = 'max' if statistic == 'max' else ('p99_upper' if histogram_bounds else 'p99')
    return {'users': len(samples), 'total_packet_samples': sum(samples),
            'samples_per_user': {'minimum': min(samples), 'maximum': max(samples)},
            'p99_sample_scope': {
                'users_whose_p99_rank_selects_maximum': sum(count < 100 for count in samples),
                'note': 'Nearest-rank p99 selects the observed maximum with 1..99 samples; histogram p99 bounds must cover that maximum. Finite observations do not establish a long-term percentile.'},
            'metrics': {key: {
                f'user_{metric}_ms': {'minimum': v[0], 'median': nearest(v, 50),
                                'p95': nearest(v, 95), 'p99': nearest(v, 99), 'maximum': v[-1]},
                'worst_packet_ms': maxima[key],
                ('users_with_max_at_or_below_ms_percent' if statistic == 'max' else
                 ('users_with_p99_upper_bound_at_or_below_ms_percent' if histogram_bounds else 'users_with_p99_at_or_below_ms_percent')): {
                    str(limit): 100 * sum(x <= limit for x in v) / len(v) for limit in thresholds}}
                for key, v in values.items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', action='append', required=True, metavar='LABEL=CSV')
    parser.add_argument('--output', type=Path, required=True, help='Fresh output stem for PNG, PDF and JSON; existing outputs are preserved')
    parser.add_argument('--subtitle', default='')
    parser.add_argument('--statistic', choices=['p99', 'max'], default='p99',
                        help='Compare per-user p99s or observed maxima; defaults to p99')
    parser.add_argument('--histogram-bounds', action='store_true',
                        help='Read bounded histogram columns; all cases must use that format')
    parser.add_argument('--threshold-ms', type=float, action='append', default=[],
                        help='Optional per-user latency threshold to summarize; repeat as needed')
    args = parser.parse_args()
    if args.histogram_bounds and args.statistic == 'max':
        parser.error('--histogram-bounds applies to p99; observed maxima are exact')
    if any(not math.isfinite(v) or v <= 0 for v in args.threshold_ms):
        parser.error('thresholds must be positive finite milliseconds')
    outputs = {suffix: args.output.with_suffix(suffix) for suffix in ['.png', '.pdf', '.json']}
    for output in outputs.values():
        if output.exists() or output.is_symlink():
            parser.error(f'output already exists: {output}')
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    fig, axes = plt.subplots(1, 2, figsize=(13, 5.5), layout='constrained')
    summary = {'scope': 'CDFs of user-level p99s, not pooled packet percentiles. Intended-send latency includes generator backlog. Finite workload observations, not latency bounds.',
               'percentile_method': 'nearest rank', 'cases': {}}
    summary['histogram_bounds'] = args.histogram_bounds
    summary['statistic'] = args.statistic
    summary['measurement_start'] = (
        'The legacy send_to_receive field starts at the source client timestamp, '
        'which can precede socket send; it is not an isolated server-hop duration.')
    if args.statistic == 'max':
        summary['scope'] = 'CDFs of per-user observed maxima. Thresholds count users with every observed packet at or below the threshold. Finite observations, not a latency guarantee; intended-send includes generator backlog.'
    if args.histogram_bounds:
        summary['scope'] = 'CDFs of user-level histogram p99 upper bounds, not exact percentiles. Threshold results describe bounds only. Maxima are exact observed durations. Intended-send includes generator backlog.'
        summary['percentile_method'] = 'nearest rank of reported histogram upper bounds'
    for case in args.case:
        label, separator, filename = case.partition('=')
        if not separator or not label or label in summary['cases']:
            parser.error('each case needs a unique nonempty LABEL=CSV')
        path = Path(filename)
        try:
            values, samples, maxima = load(path, args.histogram_bounds, args.statistic)
        except (OSError, ValueError, KeyError) as error:
            parser.error(str(error))
        record = summarize(values, samples, maxima, args.threshold_ms, args.histogram_bounds, args.statistic)
        record.update({'csv': str(path), 'csv_sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
        summary['cases'][label] = record
        for ax, key in zip(axes, values):
            ordered = values[key]
            fractions = [100 * (i + 1) / len(ordered) for i in range(len(ordered))]
            if args.histogram_bounds:
                ax.step(ordered, fractions, where='post', label=label)
            else:
                ax.plot(ordered, fractions, label=label)
    statistic_label = 'observed maximum' if args.statistic == 'max' else ('p99 upper bound' if args.histogram_bounds else 'p99 latency')
    for ax, title in zip(axes, ['From client measurement start', 'From intended send, including backlog']):
        linear = args.statistic == 'max' and ax.dataLim.xmin > 0 and ax.dataLim.xmax < 10 * ax.dataLim.xmin
        if linear:
            ax.set_xscale('linear')
        else:
            ax.set_xscale('symlog', linthresh=1)
        units = 'ms' if linear else 'ms; linear below 1 ms'
        ax.set(ylim=(0, 100), title=title,
               xlabel=f"Each user's {statistic_label} ({units})",
               ylabel='Users at or below latency (%)')
        ax.set_xlim(left=max(0, ax.get_xlim()[0]) if args.statistic == 'max' else 0)
        ax.grid(alpha=.2)
        ax.legend(fontsize=8)
    fig.suptitle(args.subtitle or f'Per-user {statistic_label} comparison')
    for suffix in ['.png', '.pdf']:
        fig.savefig(outputs[suffix], dpi=180)
    outputs['.json'].write_text(json.dumps(summary, indent=2) + '\n')


if __name__ == '__main__':
    main()
