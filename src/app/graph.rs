use egui::{Color32, Slider, SliderClamping};
use egui_plot::{Bar, BarChart, Legend, Line, Plot, PlotPoints};
use std::collections::VecDeque;

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
            num_bins: 0,      // 0 means auto
            max_bins: 100,    // Default maximum bins
        }
    }
}

pub fn show_line_graph(
    ui: &mut egui::Ui,
    values: &VecDeque<f64>,
    reverse_graph: bool,
    graph_line_color: Color32,
    mem_depth: usize,
) {
    let values: Vec<f64> = values.iter().copied().collect();
    let points: Vec<f64> = if reverse_graph {
        values.into_iter().rev().collect()
    } else {
        values
    };
    let line = Line::new("Graph", PlotPoints::from_ys_f64(&points))
        .stroke(egui::Stroke::new(2.0, graph_line_color));
    let plot = Plot::new("graph")
        .legend(Legend::default())
        .y_axis_min_width(4.0)
        .show_axes(true)
        .show_grid(true)
        .height(400.0);
    plot.show(ui, |plot_ui| {
        // Get current bounds to base our adjustments on
        let current_bounds = plot_ui.plot_bounds();
        // Set exact x-axis bounds (same for both directions; reverse_graph affects data order)
        let new_bounds = egui_plot::PlotBounds::from_min_max(
            [0.0, current_bounds.min()[1]], // x=0 is most recent (if reversed) or oldest
            [mem_depth as f64, current_bounds.max()[1]], // x=mem_depth is oldest (if reversed) or most recent
        );
        plot_ui.set_plot_bounds(new_bounds);
        // Disable x-axis autoscaling, enable y-axis autoscaling
        plot_ui.set_auto_bounds([false, true]);
        plot_ui.line(line);
    });
}

pub fn show_histogram(
    ui: &mut egui::Ui,
    hist_values: &mut VecDeque<f64>,
    curr_meas: f64,
    metermode: MeterMode,
    graph_config: &mut GraphConfig,
) {
    // Format the latest measurement for display
    let (_formatted_value, display_unit) = crate::helpers::format_measurement(
        curr_meas,
        10,
        1_000_000.0,
        0.0001,
        &metermode,
    );

    // Create bar chart data
    let hist_values_vec: Vec<f64> = hist_values.iter().copied().collect();
    let (bar_chart, max_count) = if hist_values_vec.is_empty() {
        // Create an empty bar chart to avoid plot errors
        (BarChart::new("Histogram".to_string(), vec![]), 0.0)
    } else {
        // Calculate min and max for binning
        let (min, max) = hist_values_vec.iter().fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(min, max), &x| (min.min(x), max.max(x)),
        );
        // Ensure valid range
        let range = if min == max { min - 0.5..=max + 0.5 } else { min..=max };
        let range_width = *range.end() - *range.start();

        // Determine number of bins
        let num_bins = if graph_config.num_bins == 0 {
            // Auto-bin using square root rule as a simple heuristic
            (hist_values_vec.len() as f64).sqrt().ceil() as usize
        } else {
            graph_config.num_bins
        };
        let num_bins = num_bins.max(1); // Ensure at least one bin
        let bin_width = range_width / num_bins as f64;

        // Create bins
        let mut counts = vec![0; num_bins];
        for &value in &hist_values_vec {
            if value >= *range.start() && value <= *range.end() {
                let bin_index = ((value - *range.start()) / bin_width).floor() as usize;
                let bin_index = bin_index.min(num_bins - 1); // Clamp to last bin
                counts[bin_index] += 1;
            }
        }

        // Create bars and compute max count
        let mut max_count = 0.0;
        let bars: Vec<Bar> = counts
            .into_iter()
            .enumerate()
            .map(|(i, count)| {
                let count_f64 = count as f64;
                if count_f64 > max_count {
                    max_count = count_f64;
                }
                let bin_start = *range.start() + i as f64 * bin_width;
                Bar::new(bin_start + bin_width / 2.0, count_f64)
                    .width(bin_width * 0.95) // Slight gap between bars
                    .fill(Color32::from_rgb(0, 255, 255)) // Match default graph_line_color
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(255, 255, 255)))
            })
            .collect();

        (
            BarChart::new("Histogram".to_string(), bars).color(Color32::from_rgb(0, 255, 255)), // For legend
            max_count,
        )
    };

    // Plot the bar chart
    let plot = Plot::new("histogram")
        .height(400.0)
        .show_axes(true)
        .show_grid(true)
        .y_axis_label(format!("Count ({})", display_unit))
        .x_axis_label("Value")
        .allow_scroll(false); // Prevent scrolling to keep bins stable

    plot.show(ui, |plot_ui| {
        // Set bounds to ensure proper scaling
        if !hist_values_vec.is_empty() {
            let (min, max) = hist_values_vec.iter().fold(
                (f64::INFINITY, f64::NEG_INFINITY),
                |(min, max), &x| (min.min(x), max.max(x)),
            );
            // Add padding to x-axis for better visibility
            let padding = if min == max { 0.5 } else { (max - min) * 0.05 };
            let x_bounds = [min - padding, max + padding];
            // Y-axis should be positive and include max count
            let y_bounds = [0.0, max_count * 1.1]; // 10% padding on top
            plot_ui.set_plot_bounds(egui_plot::PlotBounds::from_min_max(x_bounds, y_bounds));
        }
        plot_ui.bar_chart(bar_chart);
    });

    // Histogram controls
    ui.separator();
    ui.label("Histogram Adjustments");
    ui.horizontal(|ui| {
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

        // Reset button
        if ui.button("Reset Histogram").clicked() {
            hist_values.clear();
        }
    });
}

impl super::MyApp {
    // Update histogram buffer with new measurement
    pub fn update_histogram(&mut self, meas: f64) {
        if !meas.is_nan() {
            self.hist_values.push_back(meas);
            // Respect mem_depth for histogram as well
            while self.hist_values.len() > self.mem_depth {
                self.hist_values.pop_front();
            }
        }
    }
}