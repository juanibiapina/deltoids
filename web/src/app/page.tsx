import { Nav } from "@/components/sections/Nav";
import { Hero } from "@/components/sections/Hero";
import { Tooling } from "@/components/sections/Tooling";
import { Features } from "@/components/sections/Features";
import { Wall } from "@/components/sections/Wall";
import { Faq } from "@/components/sections/Faq";
import { Cta } from "@/components/sections/Cta";
import { Footer } from "@/components/sections/Footer";

export default function Home() {
  return (
    <>
      <Nav />
      <main>
        <Hero />
        <Tooling />
        <Features />
        <Wall />
        <Faq />
        <Cta />
      </main>
      <Footer />
    </>
  );
}
