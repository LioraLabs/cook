# 09 — deploy with `plate`

`cook` steps *produce* artifacts: sandboxed, cached, reproducible.
`plate` steps *ship* them: side effects, deliberately **not** sandboxed —
they may touch `$HOME`, a server, a phone, a bucket.

```
recipe release: site
    plate {
        mkdir -p ./device
        cp $<site> ./device/
        echo "released ..."
    }
```

`$<site>` expands to the dep's declared outputs (all of them, space-
separated), so the push consumes the build through the DAG — no globbing
at artifacts and hoping they're fresh.

The local `./device` directory stands in for wherever you actually ship:

```
plate { scp $<site> deploy@host:/var/www/ }
plate { adb push $<apk> /data/local/tmp/ }
plate { aws s3 sync build/site "s3://my-bucket" }
```

## Cache behavior worth noticing

```
$ cook release      # builds site, pushes
$ cook release      # site: cached. push: RUNS AGAIN
```

The build is cached; the push re-runs every time — cook can't see the
device's state, so it doesn't pretend to. Artifacts are cache territory;
side effects are yours.
