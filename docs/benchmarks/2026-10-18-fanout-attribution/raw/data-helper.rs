
    match timeout(Duration::from_secs(5), socket.next())
        .await?
        .context("missing data")??
    {
        Message::Binary(b) => Ok(b),
        m => anyhow::bail!("unexpected {m:?}"),
    }
}
