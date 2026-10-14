// Spike only: run the depth model on input.f32 and write output.f32.
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (models, input, out, half) = (&args[1], &args[2], &args[3], args.get(4).is_some_and(|a| a == "half"));
    let bytes = std::fs::read(input)?;
    let data: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    let precision = if half { sidecar_ml::Precision::Auto } else { sidecar_ml::Precision::F32 };
    let mut ml = sidecar_ml::Ml::new(std::path::Path::new(models), precision)?;
    for run in 0..3 {
        let t = std::time::Instant::now();
        let result = ml.depth_raw(data.clone())?;
        eprintln!("run {run}: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        if run == 2 {
            std::fs::write(out, result.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>())?;
        }
    }
    Ok(())
}
