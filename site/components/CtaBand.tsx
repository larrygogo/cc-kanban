import { REPO } from "@/lib/site";
import { getLatestRelease } from "@/lib/release";
import { getDict, withLang, type Lang } from "@/lib/i18n";
import DownloadButton from "./DownloadButton";

type Props = {
  lang?: Lang;
  title?: string;
  subtitle?: string;
};

export default async function CtaBand({ lang = "zh", title, subtitle }: Props) {
  const release = await getLatestRelease();
  const d = getDict(lang);

  return (
    <section className="section-sm">
      <div className="container">
        <div className="cta-band">
          <h2 className="h1">{title ?? d.cta.title}</h2>
          <p>{subtitle ?? d.cta.subtitle}</p>
          <div className="hero-cta" style={{ marginTop: 0 }}>
            <DownloadButton
              lang={lang}
              windows={release?.windows ?? null}
              macosArm={release?.macosArm ?? null}
              macosIntel={release?.macosIntel ?? null}
              fallbackHref={withLang(lang, "/download")}
            />
            <a
              className="btn btn-ghost btn-lg"
              href={REPO}
              target="_blank"
              rel="noopener noreferrer"
            >
              {d.cta.star}
            </a>
          </div>
        </div>
      </div>
    </section>
  );
}
