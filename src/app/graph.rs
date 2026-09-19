use egui::{Color32, RichText, Slider, SliderClamping};
use egui_plot::{
    AxisHints, Bar, BarChart, GridMark, HLine, HPlacement, HoverPosition, Legend, Line, LineStyle,
    Plot, PlotPoints,
};
use std::collections::VecDeque;

use crate::helpers::is_meter_overload;
use crate::multimeter::MeterMode;

// Configuration for graph settings
#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GraphConfig {
    pub num_bins: usize, // Number of bins for histogram, 0 for auto
    pub max_bins: usize, // Maximum number of bins for slider
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            num_bins: 0,   // 0 means auto
            max_bins: 100, // Default maximum bins
        }
    }
}

type PlotRun = Vec<[f64; 2]>;

/// Split a sample series into measurement runs and overload runs.
/// OL points use the last (or first) valid Y so they stay in axis range; they
/// are never NaN (egui panics on NaN paths).
fn split_meas_and_ol(ys: &[f64], mode: MeterMode) -> (Vec<PlotRun>, Vec<PlotRun>) {
    let fallback = ys
        .iter()
        .copied()
        .find(|y| y.is_finite() && !is_meter_overload(*y, mode))
        .unwrap_or(0.0);
    let mut meas_runs: Vec<PlotRun> = Vec::new();
    let mut ol_runs: Vec<PlotRun> = Vec::new();
    let mut meas: PlotRun = Vec::new();
    let mut ol: PlotRun = Vec::new();
    let mut last_valid = fallback;
    for (i, &y) in ys.iter().enumerate() {
        let x = i as f64;
        if !y.is_finite() {
            if !meas.is_empty() {
                meas_runs.push(std::mem::take(&mut meas));
            }
            if !ol.is_empty() {
                ol_runs.push(std::mem::take(&mut ol));
            }
        } else if is_meter_overload(y, mode) {
            if !meas.is_empty() {
                meas_runs.push(std::mem::take(&mut meas));
            }
            ol.push([x, last_valid]);
        } else {
            if !ol.is_empty() {
                ol_runs.push(std::mem::take(&mut ol));
            }
            last_valid = y;
            meas.push([x, y]);
        }
    }
    if !meas.is_empty() {
        meas_runs.push(meas);
    }
    if !ol.is_empty() {
        ol_runs.push(ol);
    }
    (meas_runs, ol_runs)
}

/// One screen-space segment per OL run so dash length is even in pixels.
/// Per-sample vertices would restart the dash pattern on serial jitter.
fn flatten_ol_run(run: &[[f64; 2]]) -> PlotRun {
    match run {
        [] => Vec::new(),
        [p] => vec![[p[0] - 0.4, p[1]], [p[0] + 0.4, p[1]]],
        [first, .., last] => vec![*first, *last],
    }
}

#[allow(clippy::too_many_arguments)]
pub fn show_line_graph(
    ui: &mut egui::Ui,
    values: &mut VecDeque<f64>,
    reverse_graph: bool,
    graph_line_color: Color32,
    mem_depth: &mut usize,
    graph_update_interval_ms: &mut u64,
    reverse_graph_mut: &mut bool,
    mem_depth_max: usize,
    graph_update_interval_max: u64,
    curr_unit: &str,
    metermode: MeterMode,
) {
    let mut ys: Vec<f64> = values.iter().copied().collect();
    if reverse_graph {
        ys.reverse();
    }
    let (meas_runs, ol_runs) = split_meas_and_ol(&ys, metermode);
    let plot = Plot::new("graph")
        .legend(Legend::default().text_style(egui::TextStyle::Monospace))
        .y_axis_min_width(4.0)
        .y_axis_label(curr_unit)
        .x_axis_label("Samples")
        .show_axes(true)
        .show_grid(true);

    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        // Graph controls directly below the graph
        ui.horizontal_wrapped(|ui| {
            ui.add(
                Slider::new(mem_depth, 10..=mem_depth_max)
                    .text("Memory Depth")
                    .step_by(10.0)
                    .clamping(SliderClamping::Always),
            );
            ui.add(
                Slider::new(graph_update_interval_ms, 10..=graph_update_interval_max)
                    .text("Update Interval (ms)")
                    .step_by(10.0)
                    .clamping(SliderClamping::Always),
            );
            if ui.button("Reset Graph").clicked() {
                values.clear();
            }
            ui.checkbox(reverse_graph_mut, "Reverse Graph (most recent on left)");
        });
        ui.label("Graph Adjustments");
        ui.separator();
        // The graph itself
        plot.show(ui, |plot_ui| {
            // Get current bounds to base our adjustments on
            let current_bounds = plot_ui.plot_bounds();
            // Set exact x-axis bounds (same for both directions; reverse_graph affects data order)
            let new_bounds = egui_plot::PlotBounds::from_min_max(
                [0.0, current_bounds.min()[1]], // x=0 is most recent (if reversed) or oldest
                [*mem_depth as f64, current_bounds.max()[1]], // x=mem_depth is oldest (if reversed) or most recent
            );
            plot_ui.set_plot_bounds(new_bounds);
            // Disable x-axis autoscaling, enable y-axis autoscaling
            plot_ui.set_auto_bounds([false, true]);
            for (i, run) in meas_runs.into_iter().enumerate() {
                let name = if i == 0 { curr_unit } else { "" };
                plot_ui.line(
                    Line::new(format!("meas{i}"), run)
                        .name(name)
                        .stroke(egui::Stroke::new(2.0, graph_line_color)),
                );
            }
            for (i, run) in ol_runs.into_iter().enumerate() {
                let name = if i == 0 { "OVERLOAD" } else { "" };
                plot_ui.line(
                    Line::new(format!("ol{i}"), flatten_ol_run(&run))
                        .name(name)
                        .stroke(egui::Stroke::new(1.5, Color32::from_rgb(220, 50, 50)))
                        .style(LineStyle::Dashed { length: 3.0 }),
                );
            }
        });
    });
}

#[derive(Clone, Copy)]
pub struct PsuGraphStyle {
    pub set_v: f64,
    pub set_i: f64,
    pub set_p: f64,
    pub color_v: Color32,
    pub color_i: Color32,
    pub color_p: Color32,
}

pub struct PsuGraph<'a> {
    pub volt: &'a mut VecDeque<f64>,
    pub curr: &'a mut VecDeque<f64>,
    pub power: &'a mut VecDeque<f64>,
    pub set_v: f64,
    pub set_i: f64,
    pub set_p: f64,
    pub color_v: Color32,
    pub color_i: Color32,
    pub color_p: Color32,
}

/// Place `set` at this fraction of plot height when it is the scale driver.
/// V / I / P use slightly different values so the three set-lines separate
/// without lying: a 5 V trace still meets the 5 V line.
const V_SET_AT: f64 = 1.00;
const I_SET_AT: f64 = 0.96;
const P_SET_AT: f64 = 0.92;

fn axis_span(set: f64, values: &VecDeque<f64>, set_at: f64) -> f64 {
    let data_max = values.iter().copied().fold(0.0_f64, f64::max);
    let set_at = set_at.max(1e-3);
    (set / set_at).max(data_max).max(1e-6)
}

fn scaled_points(values: &VecDeque<f64>, reverse: bool, span: f64) -> PlotPoints<'static> {
    let span = span.max(1e-9);
    let mut points: Vec<f64> = values.iter().map(|v| *v / span).collect();
    if reverse {
        points.reverse();
    }
    PlotPoints::from_ys_f64(&points)
}

fn psu_hline(name: &str, y: f64, color: Color32, style: LineStyle) -> HLine {
    HLine::new(name, y)
        .stroke(egui::Stroke::new(1.5, color))
        .style(style)
}

fn format_axis_tick(span: f64, mark: GridMark) -> String {
    let value = mark.value * span;
    let step = mark.step_size * span;
    let decimals = if step > 0.0 {
        (-step.log10().round() as i32).max(0) as usize
    } else {
        3
    };
    format!("{value:.decimals$}")
}

fn qty_axis(label: &str, color: Color32, span: f64, placement: HPlacement) -> AxisHints<'static> {
    AxisHints::new_y()
        .label(RichText::new(label).color(color))
        .placement(placement)
        .min_thickness(40.0)
        .tick_label_color(color)
        .formatter(move |mark, _| format_axis_tick(span, mark))
}

/// One plot: V / I / P overlay, each with its own Y axis (normalized to setpoint).
#[allow(clippy::too_many_arguments)]
pub fn show_psu_graphs(
    ui: &mut egui::Ui,
    data: PsuGraph<'_>,
    reverse_graph: bool,
    mem_depth: &mut usize,
    graph_update_interval_ms: &mut u64,
    reverse_graph_mut: &mut bool,
    mem_depth_max: usize,
    graph_update_interval_max: u64,
) {
    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.add(
                Slider::new(mem_depth, 10..=mem_depth_max)
                    .text("Memory Depth")
                    .step_by(10.0)
                    .clamping(SliderClamping::Always),
            );
            ui.add(
                Slider::new(graph_update_interval_ms, 10..=graph_update_interval_max)
                    .text("Update Interval (ms)")
                    .step_by(10.0)
                    .clamping(SliderClamping::Always),
            );
            if ui.button("Reset Graph").clicked() {
                data.volt.clear();
                data.curr.clear();
                data.power.clear();
            }
            ui.checkbox(reverse_graph_mut, "Reverse Graph (most recent on left)");
        });
        ui.label("Graph Adjustments");
        ui.separator();

        let v_span = axis_span(data.set_v, data.volt, V_SET_AT);
        let i_span = axis_span(data.set_i, data.curr, I_SET_AT);
        let p_span = axis_span(data.set_p, data.power, P_SET_AT);

        let y_axes = vec![
            qty_axis("V", data.color_v, v_span, HPlacement::Left),
            qty_axis("A", data.color_i, i_span, HPlacement::Right),
            qty_axis("W", data.color_p, p_span, HPlacement::Right),
        ];

        Plot::new("psu_graph")
            .legend(Legend::default().text_style(egui::TextStyle::Monospace))
            .custom_y_axes(y_axes)
            .x_axis_label("Samples")
            .show_axes(true)
            .show_grid(true)
            .label_formatter(move |pos| match pos {
                HoverPosition::NearDataPoint {
                    plot_name,
                    position,
                    ..
                } => {
                    let y = match *plot_name {
                        "V" => format!("{:.3} V", position.y * v_span),
                        "A" => format!("{:.3} A", position.y * i_span),
                        "W" => format!("{:.3} W", position.y * p_span),
                        "Set V" => format!("Set {:.3} V", data.set_v),
                        "Set I" => format!("Set {:.3} A", data.set_i),
                        "Set P" => format!("Set {:.3} W", data.set_p),
                        other => other.to_owned(),
                    };
                    Some(format!("{y}\nsample {:.0}", position.x))
                }
                HoverPosition::Elsewhere { position } => Some(format!(
                    "{:.3} V\n{:.3} A\n{:.3} W",
                    position.y * v_span,
                    position.y * i_span,
                    position.y * p_span
                )),
            })
            .show(ui, |plot_ui| {
                let new_bounds =
                    egui_plot::PlotBounds::from_min_max([0.0, 0.0], [*mem_depth as f64, 1.08]);
                plot_ui.set_plot_bounds(new_bounds);
                plot_ui.set_auto_bounds([false, false]);
                plot_ui.line(
                    Line::new("V", scaled_points(data.volt, reverse_graph, v_span))
                        .stroke(egui::Stroke::new(2.0, data.color_v)),
                );
                plot_ui.line(
                    Line::new("A", scaled_points(data.curr, reverse_graph, i_span))
                        .stroke(egui::Stroke::new(2.0, data.color_i)),
                );
                plot_ui.line(
                    Line::new("W", scaled_points(data.power, reverse_graph, p_span))
                        .stroke(egui::Stroke::new(2.0, data.color_p)),
                );
                plot_ui.hline(psu_hline(
                    "Set V",
                    data.set_v / v_span,
                    data.color_v,
                    LineStyle::dashed_dense(),
                ));
                plot_ui.hline(psu_hline(
                    "Set I",
                    data.set_i / i_span,
                    data.color_i,
                    LineStyle::dashed_loose(),
                ));
                plot_ui.hline(psu_hline(
                    "Set P",
                    data.set_p / p_span,
                    data.color_p,
                    LineStyle::dotted_dense(),
                ));
            });
    });
}

#[allow(clippy::too_many_arguments)]
pub fn show_histogram(
    ui: &mut egui::Ui,
    hist_values: &mut VecDeque<f64>,
    curr_meas: f64,
    metermode: MeterMode,
    graph_config: &mut GraphConfig,
    hist_bar_color: Color32,
    hist_collect_active: &mut bool,
    hist_collect_interval_ms: &mut u64,
    hist_mem_depth: &mut usize,
    hist_mem_depth_max: usize,
) {
    // Format the latest measurement for display
    let (_formatted_value, display_unit) = crate::helpers::format_measurement(
        curr_meas,
        10,
        1_000_000.0,
        0.0001,
        &metermode,
        false,
        None,
    );

    // Create bar chart data
    let hist_values_vec: Vec<f64> = hist_values
        .iter()
        .copied()
        .filter(|y| y.is_finite() && !is_meter_overload(*y, metermode))
        .collect();
    let (bar_chart, max_count, _num_bins, _bin_width, _range_start, _range_end) =
        if hist_values_vec.is_empty() {
            (
                BarChart::new("Histogram (0 values, bin width: 0)".to_string(), vec![]),
                0.0,
                0,
                0.0,
                0.0,
                0.0,
            )
        } else {
            // Calculate min and max for binning
            let (min, max) = hist_values_vec
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), &x| {
                    (min.min(x), max.max(x))
                });
            // Ensure valid range, handle single-value case
            let range_width = if min == max {
                if min == 0.0 {
                    1.0 // Avoid zero range for zero values
                } else {
                    min.abs() * 0.1 // 10% of value for single value
                }
            } else {
                max - min
            };
            let range_start = if min == max {
                min - range_width / 2.0
            } else {
                min
            };
            let range_end = range_start + range_width;

            // Determine number of bins
            let num_bins = if graph_config.num_bins == 0 {
                // Auto-bin using square root rule, capped at max_bins
                let sqrt_bins = (hist_values_vec.len() as f64).sqrt().ceil() as usize;
                sqrt_bins.min(graph_config.max_bins).max(1) // Ensure at least one bin
            } else {
                graph_config.num_bins.max(1) // Ensure at least one bin
            };

            // Calculate bin width in data units
            let bin_width = range_width / num_bins as f64;

            // Create bins
            let mut counts = vec![0; num_bins];
            for &value in &hist_values_vec {
                if value >= range_start && value <= range_end {
                    let bin_index = ((value - range_start) / bin_width).floor() as usize;
                    let bin_index = bin_index.min(num_bins - 1); // Clamp to last bin
                    counts[bin_index] += 1;
                }
            }

            // Compute max_count separately
            let max_count = *counts.iter().max().unwrap_or(&0) as f64;

            // Format bin width for legend
            let (formatted_bin_width, bin_width_unit) = crate::helpers::format_measurement(
                bin_width,
                10,
                1_000_000.0,
                0.0001,
                &metermode,
                false,
                None,
            );
            let chart_name = format!(
                "  Samples: {}\nBin Width: {} {}\n      Min: {}\n      Max: {}",
                hist_values_vec.len(),
                formatted_bin_width.trim_start(),
                bin_width_unit,
                min,
                max
            );

            // Create bars in normalized canvas coordinates (0 to num_bins)
            let display_bar_width = 1.0; // Width of 1.0 in normalized units
            let bars: Vec<Bar> = counts
                .into_iter()
                .enumerate()
                .map(|(i, count)| {
                    let count_f64 = count as f64;
                    // Center the bar at i + 0.5 in normalized coordinates
                    let bar_center = i as f64 + 0.5;
                    // Directly initialize stroke based on theme
                    let stroke = if ui.ctx().theme().default_visuals().dark_mode {
                        egui::Stroke::new(0.5, Color32::from_rgb(255, 255, 255))
                    } else {
                        egui::Stroke::new(0.5, Color32::from_rgb(0, 0, 0))
                    };
                    Bar::new(bar_center, count_f64)
                        .width(display_bar_width * 0.95) // Slight gap between bars
                        .fill(hist_bar_color)
                        .stroke(stroke)
                })
                .collect();

            // Define element formatter for hover tooltip
            let formatter = Box::new(move |bar: &Bar, _chart: &BarChart| {
                // Calculate bin index from bar center (subtract 0.5 to get zero-based index)
                let bin_index = (bar.argument - 0.5).floor() as usize;
                // Calculate bin range
                let bin_start = range_start + bin_index as f64 * bin_width;
                let bin_end = bin_start + bin_width;
                // Format bin start and end using the same formatting as measurements
                let (formatted_start, _) = crate::helpers::format_measurement(
                    bin_start,
                    10,
                    1_000_000.0,
                    0.0001,
                    &metermode,
                    false,
                    None,
                );
                let (formatted_end, _) = crate::helpers::format_measurement(
                    bin_end,
                    10,
                    1_000_000.0,
                    0.0001,
                    &metermode,
                    false,
                    None,
                );
                // Sample count is the bar's value (height)
                let sample_count = bar.value as usize;
                format!(
                    "Bin Range: {} to {} {}\nSamples: {}",
                    formatted_start.trim_start(),
                    formatted_end.trim_start(),
                    display_unit,
                    sample_count
                )
            });

            (
                BarChart::new(chart_name, bars)
                    .color(hist_bar_color)
                    .element_formatter(formatter),
                max_count,
                num_bins,
                bin_width,
                range_start,
                range_end,
            )
        };

    // Use bottom-up layout to place controls at bottom and plot above
    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
        // Diagnostic labels (bottom to top due to bottom_up layout)
        // if num_bins > 0 {
        //     let bin_ranges: Vec<String> = (0..num_bins)
        //         .map(|i| {
        //             let bin_start = range_start + i as f64 * bin_width;
        //             let bin_end = bin_start + bin_width;
        //             format!("Bin {}: {:.2} to {:.2}", i, bin_start, bin_end)
        //         })
        //         .collect();
        //     ui.label(format!("Bin ranges: {:?}", bin_ranges));
        // }
        // ui.label(format!("Max count: {}", max_count));
        // ui.label(format!("Data range: {:.2} to {:.2}", min, max));
        // ui.label(format!("Bin width (data units): {:.6}", bin_width));
        // ui.label(format!("Number of bins: {}", num_bins));
        // ui.separator();

        ui.horizontal_wrapped(|ui| {
            // Histogram memory depth slider
            ui.add(
                Slider::new(hist_mem_depth, 100..=hist_mem_depth_max)
                    .text("Memory Depth")
                    .step_by(100.0)
                    .clamping(SliderClamping::Always),
            );

            // Reset button
            if ui.button("Reset Histogram").clicked() {
                hist_values.clear();
            }

            // Start/Stop collection button
            if ui
                .button(if *hist_collect_active {
                    "Stop Collection"
                } else {
                    "Start Collection"
                })
                .clicked()
            {
                *hist_collect_active = !*hist_collect_active;
            }

            // Number of bins slider
            let num_bins_label = if graph_config.num_bins == 0 {
                "Bins: Auto".to_string()
            } else {
                format!("Bins: {}", graph_config.num_bins)
            };
            ui.add(
                Slider::new(&mut graph_config.num_bins, 0..=graph_config.max_bins)
                    .text(num_bins_label)
                    .step_by(1.0)
                    .clamping(SliderClamping::Always),
            );
            let mut interval_str = hist_collect_interval_ms.to_string();

            // Collection interval
            if ui
                .add(
                    egui::TextEdit::singleline(&mut interval_str)
                        .desired_width(100.0)
                        .hint_text("Collection Interval (ms)"),
                )
                .changed()
            {
                if let Ok(new_interval) = interval_str.parse::<u64>() {
                    if new_interval > 0 {
                        *hist_collect_interval_ms = new_interval;
                    }
                }
            }
            ui.label("Collection Interval (ms)");
        });
        ui.label("Histogram Adjustments");
        ui.separator();

        // Plot the histogram above controls, taking remaining space
        let plot = Plot::new("histogram")
            .show_axes(true)
            .show_grid(true)
            .y_axis_label("Count")
            .x_axis_label("Bin Index")
            .allow_scroll(false) // Prevent scrolling to keep bins stable
            .default_y_bounds(-0.1, 1.0)
            .include_y(max_count * 1.2)
            .legend(
                Legend::default()
                    .position(egui_plot::Corner::RightTop)
                    .text_style(egui::TextStyle::Monospace),
            );

        plot.show(ui, |plot_ui| {
            // Auto-scale x, do y manually to leave space for legend
            plot_ui.set_auto_bounds([true, true]);
            plot_ui.bar_chart(bar_chart);
        });
    });
}

impl super::MyApp {
    // Update histogram buffer with new measurement
    pub fn update_histogram(&mut self, meas: f64) {
        if meas.is_finite() && !is_meter_overload(meas, self.metermode) && self.hist_collect_active
        {
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();
            let hist_interval = self.hist_collect_interval_ms as f64 / 1000.0; // Convert ms to seconds
            if current_time - self.last_hist_collect_time >= hist_interval {
                self.hist_values.push_back(meas);
                // Respect hist_mem_depth for histogram
                while self.hist_values.len() > self.hist_mem_depth {
                    self.hist_values.pop_front();
                }
                self.last_hist_collect_time = current_time;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ol_run_sits_in_the_gap_at_last_valid_y() {
        let ys = [1.0, 2.0, 9.9e31, 9.9e31, 3.0];
        let (meas, ol) = split_meas_and_ol(&ys, MeterMode::Res);
        assert_eq!(meas.len(), 2);
        assert_eq!(meas[0], [[0.0, 1.0], [1.0, 2.0]]);
        assert_eq!(meas[1], [[4.0, 3.0]]);
        assert_eq!(ol.len(), 1);
        assert_eq!(ol[0], [[2.0, 2.0], [3.0, 2.0]]);
    }

    #[test]
    fn all_ol_keeps_scrolling_at_zero() {
        let ys = [9.9e31, 9.9e31];
        let (meas, ol) = split_meas_and_ol(&ys, MeterMode::Adc);
        assert!(meas.is_empty());
        assert_eq!(ol, vec![vec![[0.0, 0.0], [1.0, 0.0]]]);
    }

    #[test]
    fn one_gigahertz_is_not_an_ol_run() {
        let ys = [1e9];
        let (meas, ol) = split_meas_and_ol(&ys, MeterMode::Freq);
        assert_eq!(meas, vec![vec![[0.0, 1e9]]]);
        assert!(ol.is_empty());
    }

    #[test]
    fn ol_overlay_is_one_segment_not_per_sample() {
        let ys = [1.0, 9.9e31, 9.9e31, 9.9e31, 2.0];
        let (_, ol) = split_meas_and_ol(&ys, MeterMode::Res);
        assert_eq!(ol[0].len(), 3);
        assert_eq!(flatten_ol_run(&ol[0]), [[1.0, 1.0], [3.0, 1.0]]);
    }
}
