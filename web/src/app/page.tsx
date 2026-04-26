import { Nav } from "@/components/sections/Nav";
import { Hero } from "@/components/sections/Hero";
import { Pager } from "@/components/sections/Pager";
import { Agents } from "@/components/sections/Agents";
import { Faq } from "@/components/sections/Faq";
import { Footer } from "@/components/sections/Footer";
import { getDeltoidsStars } from "@/lib/github";

export default async function Home() {
  const stars = await getDeltoidsStars();
  return (
    <>
      <Nav stars={stars} />
      <main>
        <Hero />
        <Pager />
        <Agents />
        <Faq />
      </main>
      <Footer />
    </>
  );
}
