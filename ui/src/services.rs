use auro::prelude::*;

#[auro::command]
pub async fn increment(current: u64) -> Result<u64, AuroError> {
    Ok(current + 1)
}
